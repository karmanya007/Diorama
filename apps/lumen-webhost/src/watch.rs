//! Telling the user why a web wallpaper misbehaves: files the page could not
//! load (a library from a CDN with the network off leaves pages blank or
//! stuck) and a page that stopped answering (a script in an endless loop).
//! Both go to the log, which lumend shows.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

/// How often a page is asked to answer, and how long it may take.
const PING_EVERY: Duration = Duration::from_secs(5);
const STUCK_AFTER: Duration = Duration::from_secs(10);
/// Distinct script errors logged per page process (pages can throw every frame).
const MAX_ERRORS: usize = 20;

/// What the pages reported, shared by every output's page.
#[derive(Clone)]
pub struct Reports {
    inner: Arc<Inner>,
}

struct Inner {
    folder: PathBuf,
    allow_network: bool,
    seen: Mutex<HashSet<String>>,
    /// Network files that failed while the network is off.
    blocked: AtomicUsize,
    errors: AtomicUsize,
}

impl Reports {
    pub fn new(folder: PathBuf, allow_network: bool) -> Self {
        // Shown in a command to copy: without the `\\?\` canonicalize adds.
        let folder = folder
            .to_str()
            .and_then(|s| s.strip_prefix(r"\\?\"))
            .map_or(folder.clone(), PathBuf::from);
        Self {
            inner: Arc::new(Inner {
                folder,
                allow_network,
                seen: Mutex::new(HashSet::new()),
                blocked: AtomicUsize::new(0),
                errors: AtomicUsize::new(0),
            }),
        }
    }

    /// One message from the page script (`window.ipc.postMessage`).
    pub fn handle(&self, message: &str) {
        let Ok(msg) = serde_json::from_str::<Value>(message) else {
            return;
        };
        let text = |k: &str| msg[k].as_str().unwrap_or_default().to_owned();
        match msg["kind"].as_str() {
            Some("load-failed") => {
                let url = text("url");
                if !self.first(format!("load {url}")) {
                    return;
                }
                if self.blocked_by_us(&url) {
                    self.inner.blocked.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(
                        "web wallpaper could not load {} {url}: the network is off for this \
                         wallpaper, and pages that need such files can stay blank or freeze \
                         (allow it: lumen set \"{}\" --allow-network)",
                        text("tag"),
                        self.inner.folder.display()
                    );
                } else {
                    tracing::warn!("web wallpaper could not load {} {url}", text("tag"));
                }
            }
            Some("error") => {
                let message = text("message");
                if self.inner.errors.load(Ordering::Relaxed) >= MAX_ERRORS
                    || !self.first(format!("error {message}"))
                {
                    return;
                }
                self.inner.errors.fetch_add(1, Ordering::Relaxed);
                tracing::debug!(
                    source = text("source"),
                    line = msg["line"].as_u64().unwrap_or(0),
                    "web wallpaper script error: {message}"
                );
            }
            _ => {}
        }
    }

    /// Why a page might be stuck, for the watchdog's warning.
    pub fn hint(&self) -> String {
        match self.inner.blocked.load(Ordering::Relaxed) {
            0 => String::new(),
            n => format!(
                "; it could not load {n} file(s) from the network, which is off for this \
                 wallpaper (allow it: lumen set \"{}\" --allow-network)",
                self.inner.folder.display()
            ),
        }
    }

    fn first(&self, key: String) -> bool {
        self.inner.seen.lock().is_ok_and(|mut s| s.insert(key))
    }

    /// A network address (not the wallpaper's own files) with the network off.
    fn blocked_by_us(&self, url: &str) -> bool {
        !self.inner.allow_network
            && (url.starts_with("http://") || url.starts_with("https://"))
            && !url.starts_with("https://lumen.")
    }
}

/// Asks a page to answer every few seconds and warns once while it does not.
#[derive(Default)]
pub struct Watchdog {
    answered: Arc<AtomicBool>,
    asked: Option<Instant>,
    next: Option<Instant>,
    stuck: bool,
}

impl Watchdog {
    /// `ask` sends a script whose completion calls the given callback.
    pub fn tick(&mut self, reports: &Reports, ask: impl FnOnce(Box<dyn Fn() + Send>)) {
        let now = Instant::now();
        if let Some(asked) = self.asked {
            if self.answered.load(Ordering::SeqCst) {
                if self.stuck {
                    tracing::info!("web wallpaper page is responding again");
                    self.stuck = false;
                }
                self.asked = None;
                self.next = Some(asked + PING_EVERY);
            } else if !self.stuck && now.duration_since(asked) > STUCK_AFTER {
                self.stuck = true;
                tracing::warn!(
                    "web wallpaper page stopped responding (a script is probably stuck in a \
                     loop){}",
                    reports.hint()
                );
            }
            return;
        }
        if self.next.is_some_and(|t| now < t) {
            return;
        }
        self.answered.store(false, Ordering::SeqCst);
        self.asked = Some(now);
        let answered = Arc::clone(&self.answered);
        ask(Box::new(move || answered.store(true, Ordering::SeqCst)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_failures_count_only_when_we_blocked_them() {
        let off = Reports::new(PathBuf::from("C:/w"), false);
        off.handle(r#"{"kind":"load-failed","tag":"script","url":"https://cdn.example/p5.js"}"#);
        off.handle(r#"{"kind":"load-failed","tag":"script","url":"https://cdn.example/p5.js"}"#);
        off.handle(r#"{"kind":"load-failed","tag":"script","url":"https://lumen.w1/missing.js"}"#);
        assert!(off.hint().contains("1 file(s)"), "{}", off.hint());
        let on = Reports::new(PathBuf::from("C:/w"), true);
        on.handle(r#"{"kind":"load-failed","tag":"script","url":"https://cdn.example/p5.js"}"#);
        assert_eq!(on.hint(), "");
        on.handle("not json");
    }

    #[test]
    fn watchdog_asks_then_waits_for_the_answer() {
        let reports = Reports::new(PathBuf::from("C:/w"), true);
        let mut dog = Watchdog::default();
        let mut reply = None;
        dog.tick(&reports, |done| reply = Some(done));
        let reply = reply.expect("asked at once");
        let mut again = false;
        dog.tick(&reports, |_| again = true);
        assert!(!again, "no second question while the first is open");
        reply();
        dog.tick(&reports, |_| again = true);
        assert!(!again, "next question waits {PING_EVERY:?}");
        assert!(dog.asked.is_none() && !dog.stuck);
    }
}
