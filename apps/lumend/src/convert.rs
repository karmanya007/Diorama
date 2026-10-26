//! Copies of videos converted for hardware decoding (P4), kept in
//! `<cache dir>/videos` and played instead of the original (whose soundtrack
//! still plays). Conversions run on their own threads.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;

use lumen_ipc::Conversion;

/// Where the converted copy of `video` goes: named after its path, size and
/// modification time, so a changed original is converted afresh.
pub fn copy_path(video: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(video).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let key = format!(
        "{}|{}|{modified}",
        video.to_string_lossy().to_lowercase().replace('\\', "/"),
        meta.len()
    );
    Some(
        lumen_platform::cache_dir()?
            .join("videos")
            .join(format!("{:016x}.mp4", fnv1a(key.as_bytes()))),
    )
}

/// The converted copy of `video`, if one was made.
pub fn existing_copy(video: &Path) -> Option<PathBuf> {
    copy_path(video).filter(|p| p.is_file())
}

/// FNV-1a: stable across builds, unlike std's hasher.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

struct Job {
    path: PathBuf,
    /// Fraction done, as `f32` bits.
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<(), String>>>,
    error: Option<String>,
}

#[derive(Default)]
pub struct Conversions {
    jobs: Vec<Job>,
}

impl Conversions {
    /// Starts converting `video` to `size`; `done` is called (from the
    /// conversion's thread) when it ends.
    pub fn start(
        &mut self,
        video: &Path,
        size: (u32, u32),
        done: impl FnOnce() + Send + 'static,
    ) -> anyhow::Result<()> {
        if self
            .jobs
            .iter()
            .any(|j| j.path == video && j.thread.is_some())
        {
            return Ok(());
        }
        self.jobs.retain(|j| j.path != video);
        let dst = copy_path(video).ok_or_else(|| anyhow::anyhow!("no cache folder"))?;
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let thread = {
            let (src, progress, cancel) = (video.to_path_buf(), progress.clone(), cancel.clone());
            std::thread::Builder::new()
                .name("convert".into())
                .spawn(move || {
                    let result = convert(&src, &dst, size, &progress, &cancel);
                    done();
                    result
                })?
        };
        tracing::info!(video = %video.display(), ?size, "converting for hardware decoding");
        self.jobs.push(Job {
            path: video.to_path_buf(),
            progress,
            cancel,
            thread: Some(thread),
            error: None,
        });
        Ok(())
    }

    /// Videos whose conversion finished successfully since the last call.
    pub fn finished(&mut self) -> Vec<PathBuf> {
        let mut done = Vec::new();
        for job in &mut self.jobs {
            if !job.thread.as_ref().is_some_and(JoinHandle::is_finished) {
                continue;
            }
            let result = job.thread.take().map(|t| {
                t.join()
                    .unwrap_or_else(|_| Err("conversion crashed".into()))
            });
            match result {
                Some(Ok(())) => done.push(job.path.clone()),
                Some(Err(e)) => {
                    tracing::warn!(video = %job.path.display(), "conversion failed: {e}");
                    job.error = Some(e);
                }
                None => {}
            }
        }
        // Finished ones are reported once; failures stay listed.
        self.jobs
            .retain(|j| j.thread.is_some() || j.error.is_some());
        done
    }

    pub fn status(&self) -> Vec<Conversion> {
        self.jobs
            .iter()
            .map(|j| Conversion {
                path: j.path.clone(),
                progress: f32::from_bits(j.progress.load(Ordering::Relaxed)),
                error: j.error.clone(),
            })
            .collect()
    }
}

impl Drop for Conversions {
    fn drop(&mut self) {
        for job in &self.jobs {
            job.cancel.store(true, Ordering::Relaxed);
        }
        for job in &mut self.jobs {
            if let Some(t) = job.thread.take() {
                let _ = t.join();
            }
        }
    }
}

/// Converts into a temporary file first: a half-written copy is never played.
fn convert(
    src: &Path,
    dst: &Path,
    size: (u32, u32),
    progress: &AtomicU32,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let dir = dst.parent().ok_or("no cache folder")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    // The container is picked by the extension.
    let part = dst.with_extension("part.mp4");
    let result = lumen_media::convert(src, &part, size, &mut |p| {
        progress.store(p.to_bits(), Ordering::Relaxed);
        !cancel.load(Ordering::Relaxed)
    });
    match result {
        Ok(()) => std::fs::rename(&part, dst).map_err(|e| e.to_string()),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn copy_names_are_stable() {
        // FNV-1a test vectors.
        assert_eq!(super::fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(super::fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
