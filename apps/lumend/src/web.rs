//! Web wallpapers: a `lumen-webhost` process shows the page; lumend feeds it
//! the user's property choices, pause state, audio, media and the cursor as
//! JSON lines on its stdin (closing stdin ends it).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use lumen_platform::Platform;
use serde_json::{Map, Value, json};

use crate::media::{MediaChange, MediaTracker};
use crate::session::SceneAudio;

/// WE feeds web audio listeners about 30 times a second.
const AUDIO_INTERVAL: Duration = Duration::from_millis(33);
/// Let the page load and settle before photographing it for the OS fallback.
const SNAPSHOT_AFTER: Duration = Duration::from_secs(5);
/// Give up on a snapshot the host never wrote.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(30);

/// Progress of [`WebHost::snapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Snapshot {
    Pending,
    Ready,
    Failed,
}

/// A web wallpaper folder and its page.
#[derive(Debug, Clone)]
pub struct WebSource {
    pub dir: PathBuf,
    /// Page relative to `dir`.
    pub file: String,
    pub we_config: Option<PathBuf>,
}

impl WebSource {
    /// From a project folder, its `project.json`, or an HTML page.
    pub fn new(path: &Path, we_config: Option<&Path>) -> Result<Self> {
        let (dir, file) = if path.is_dir() {
            (path.to_path_buf(), None)
        } else {
            let dir = path
                .parent()
                .context("page without a folder")?
                .to_path_buf();
            let name = path.file_name().and_then(|n| n.to_str()).map(str::to_owned);
            let is_project = name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case("project.json"));
            (dir, name.filter(|_| !is_project))
        };
        let file = match file {
            Some(f) => f,
            None => project_file(&dir).unwrap_or_else(|| "index.html".into()),
        };
        anyhow::ensure!(
            dir.join(&file).is_file(),
            "{} has no page {file}",
            dir.display()
        );
        Ok(Self {
            dir,
            file,
            we_config: we_config.map(Path::to_path_buf),
        })
    }

    /// The user's property choices from Wallpaper Engine (keyed by the page).
    fn choices(&self) -> Map<String, Value> {
        self.we_config
            .as_deref()
            .map(|c| lumen_we::config::user_properties_from_file(c, &self.dir.join(&self.file), 0))
            .unwrap_or_default()
            .into_iter()
            .collect()
    }
}

/// `file` from the folder's `project.json`.
fn project_file(dir: &Path) -> Option<String> {
    let bytes = std::fs::read(dir.join("project.json")).ok()?;
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    let v: Value = serde_json::from_slice(bytes).ok()?;
    Some(v.get("file")?.as_str()?.to_owned())
}

/// `lumen-webhost` next to this executable (or `LUMEN_WEBHOST`).
fn webhost_exe() -> PathBuf {
    if let Some(p) = std::env::var_os("LUMEN_WEBHOST") {
        return PathBuf::from(p);
    }
    let name = format!("lumen-webhost{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(&name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

pub struct WebHost {
    child: Child,
    stdin: Option<ChildStdin>,
    media: MediaTracker,
    audio: Option<SceneAudio>,
    started: Instant,
    next_audio: Instant,
    /// Every output paused: no audio, media or cursor updates.
    paused: bool,
    /// Pause per output id, as last sent.
    output_pauses: Vec<(String, bool)>,
    /// One page across all outputs: it pauses only when all of them do.
    span: bool,
    /// Last cursor sent (desktop pixels) and whether the button was down.
    pointer: Option<(i32, i32, bool)>,
    /// When the last snapshot was asked for.
    snapshot_asked: Option<std::time::SystemTime>,
}

impl WebHost {
    pub fn spawn(source: &WebSource, allow_network: bool, span: bool) -> Result<Self> {
        let exe = webhost_exe();
        let mut cmd = Command::new(&exe);
        cmd.arg(&source.dir)
            .arg("--file")
            .arg(&source.file)
            .stdin(Stdio::piped())
            .stdout(Stdio::null());
        if allow_network {
            cmd.arg("--allow-network");
        }
        if span {
            cmd.arg("--span");
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("starting {}", exe.display()))?;
        let stdin = child.stdin.take();
        let mut host = Self {
            child,
            stdin,
            media: MediaTracker::start(),
            audio: SceneAudio::start(),
            started: Instant::now(),
            next_audio: Instant::now(),
            paused: false,
            output_pauses: Vec::new(),
            span,
            pointer: None,
            snapshot_asked: None,
        };
        // The first line is the user's choices (read before the page loads).
        host.send(&json!({ "properties": source.choices() }));
        tracing::info!(
            page = %source.dir.join(&source.file).display(),
            network = allow_network,
            "web wallpaper host started"
        );
        Ok(host)
    }

    fn send(&mut self, message: &Value) {
        let Some(stdin) = self.stdin.as_mut() else {
            return;
        };
        let mut line = message.to_string();
        line.push('\n');
        if let Err(e) = stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
        {
            tracing::warn!("web wallpaper host gone: {e}");
            self.stdin = None;
        }
    }

    /// Whether the host process is still running.
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// A PNG of the page at `path`, asked for once the page had time to load:
    /// call each tick until it is not [`Snapshot::Pending`].
    pub fn snapshot(&mut self, path: &Path) -> Snapshot {
        let Some(asked) = self.snapshot_asked else {
            if self.started.elapsed() >= SNAPSHOT_AFTER {
                self.snapshot_asked = Some(std::time::SystemTime::now());
                self.send(&json!({ "snapshot": path }));
            }
            return Snapshot::Pending;
        };
        let written = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .is_ok_and(|t| t >= asked);
        if written {
            Snapshot::Ready
        } else if asked.elapsed().unwrap_or_default() > SNAPSHOT_TIMEOUT {
            Snapshot::Failed
        } else {
            Snapshot::Pending
        }
    }

    /// `(output id, paused)` for every output: pages on covered monitors
    /// stop while the others keep playing.
    pub fn set_paused(&mut self, outputs: Vec<(String, bool)>) {
        if outputs == self.output_pauses {
            return;
        }
        self.paused = !outputs.is_empty() && outputs.iter().all(|(_, p)| *p);
        if self.span {
            let all = self.paused;
            self.send(&json!({ "paused": all }));
        } else {
            let map: Map<String, Value> = outputs
                .iter()
                .map(|(id, p)| (id.clone(), Value::Bool(*p)))
                .collect();
            self.send(&json!({ "paused": map }));
        }
        self.output_pauses = outputs;
    }

    /// How long until [`WebHost::tick`] has work again.
    pub fn time_until_next(&self) -> Duration {
        if self.paused {
            Duration::from_secs(1)
        } else {
            self.next_audio.saturating_duration_since(Instant::now())
        }
    }

    /// Audio, media and cursor updates due now.
    pub fn tick(&mut self, platform: &Platform) {
        if self.paused {
            return;
        }
        let time = self.started.elapsed().as_secs_f32();
        let now = Instant::now();
        if now >= self.next_audio {
            // A fixed cadence: waits overshoot by the OS timer granularity.
            self.next_audio += AUDIO_INTERVAL;
            if self.next_audio < now {
                self.next_audio = now + AUDIO_INTERVAL;
            }
            if let Some(audio) = self.audio.as_mut() {
                let [left, right] = audio.web_spectrum(time);
                let levels: Vec<f32> = left.iter().chain(right.iter()).copied().collect();
                self.send(&json!({ "audio": levels }));
            }
        }
        for change in self.media.changes(time) {
            let (kind, event) = match change {
                MediaChange::Status { enabled } => ("status", json!({ "enabled": enabled })),
                MediaChange::Properties(p) => ("properties", p),
                MediaChange::Playback(state) => ("playback", json!({ "state": state })),
                MediaChange::Thumbnail => ("thumbnail", thumbnail_event(self.media.current())),
                MediaChange::Timeline(t) => ("timeline", t),
            };
            self.send(&json!({ "media": { "kind": kind, "event": event } }));
        }
        self.forward_pointer(platform);
    }

    /// The page sits behind the icons; forward the cursor while it is over
    /// the desktop (moves, and button changes as down/up).
    fn forward_pointer(&mut self, platform: &Platform) {
        let state = platform.desktop_pointer();
        let Some((x, y)) = platform.cursor_position() else {
            return;
        };
        if !state.over_desktop {
            // Release a press that left the desktop.
            if let Some((px, py, true)) = self.pointer {
                self.send(&json!({ "pointer": { "x": px, "y": py, "kind": "up" } }));
            }
            self.pointer = None;
            return;
        }
        let down = state.left_down;
        let kind = match self.pointer {
            Some((_, _, was)) if was != down => Some(if down { "down" } else { "up" }),
            Some((px, py, _)) if (px, py) == (x, y) => None,
            _ => Some("move"),
        };
        if let Some(kind) = kind {
            self.send(&json!({ "pointer": { "x": x, "y": y, "kind": kind } }));
        }
        self.pointer = Some((x, y, down));
    }
}

impl Drop for WebHost {
    fn drop(&mut self) {
        // Closing stdin asks the host to exit; make sure it does.
        self.stdin = None;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// WE's web thumbnail event: the album art as a data URL and its colours as
/// CSS colours.
fn thumbnail_event(m: Option<&lumen_platform::MediaInfo>) -> Value {
    let Some(bytes) = m.and_then(|m| m.thumbnail.as_ref()) else {
        return json!({ "thumbnail": "" });
    };
    let mime = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else {
        "image/jpeg"
    };
    let mut event = json!({
        "thumbnail": format!("data:{mime};base64,{}", base64(bytes)),
    });
    if let Ok(img) = image::load_from_memory(bytes) {
        let c = lumen_script::thumbnail_colors(img.thumbnail(64, 64).to_rgba8().as_raw());
        let css = |c: [f32; 3]| {
            let [r, g, b] = c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
            format!("#{r:02x}{g:02x}{b:02x}")
        };
        event["primaryColor"] = css(c.primary).into();
        event["secondaryColor"] = css(c.secondary).into();
        event["tertiaryColor"] = css(c.tertiary).into();
        event["textColor"] = css(c.text).into();
        event["highContrastColor"] = css(c.high_contrast).into();
    }
    event
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(TABLE[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn sources_from_folder_project_or_page() {
        let dir = std::env::temp_dir().join(format!("lumend-web-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("project.json"),
            r#"{"type": "web", "file": "page.html"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("page.html"), "<p>").unwrap();
        std::fs::write(dir.join("other.html"), "<p>").unwrap();
        assert_eq!(WebSource::new(&dir, None).unwrap().file, "page.html");
        assert_eq!(
            WebSource::new(&dir.join("project.json"), None)
                .unwrap()
                .file,
            "page.html"
        );
        assert_eq!(
            WebSource::new(&dir.join("other.html"), None).unwrap().file,
            "other.html"
        );
        std::fs::write(
            dir.join("project.json"),
            r#"{"type": "web", "file": "gone.html"}"#,
        )
        .unwrap();
        assert!(WebSource::new(&dir, None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
