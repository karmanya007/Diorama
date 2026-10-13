//! Media backends (L3). Like `lumen-platform`, this crate may contain
//! OS-specific code (`scripts/check-layers.sh` allows it); everything above it
//! sees only the OS-neutral types here.
//!
//! Video: decoders produce NV12 frames (luma plane + interleaved chroma at half
//! resolution), the native output of hardware decoders. The GPU converts to RGB.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::Duration;

#[cfg(windows)]
mod windows;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("video decoding is not supported on this platform yet")]
    Unsupported,
    #[error("{call} failed: {message}")]
    Os { call: &'static str, message: String },
    #[error("no video stream in {0}")]
    NoVideo(PathBuf),
    #[error("decoder produced an invalid frame: {0}")]
    BadFrame(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    /// Frames per second, if the container declares it.
    pub frame_rate: Option<f64>,
}

/// One decoded frame in NV12 layout.
#[derive(Debug, Clone)]
pub struct Nv12Frame {
    pub width: u32,
    pub height: u32,
    /// Bytes per row of both planes (chroma rows hold `width/2` UV pairs).
    pub stride: u32,
    /// Luma plane (`stride * height` bytes) followed by the chroma plane
    /// (`stride * height/2` bytes) at `chroma_offset`.
    pub data: Vec<u8>,
    pub chroma_offset: usize,
    /// Presentation time from the start of playback (accumulates across loops).
    pub pts: Duration,
}

impl Nv12Frame {
    pub fn luma(&self) -> &[u8] {
        &self.data[..self.chroma_offset.min(self.data.len())]
    }

    pub fn chroma(&self) -> &[u8] {
        &self.data[self.chroma_offset.min(self.data.len())..]
    }
}

/// A decoder for one file. Implementations live in the per-OS modules.
pub trait VideoDecoder: Send {
    fn info(&self) -> VideoInfo;
    /// Next frame in presentation order, or `None` at end of stream.
    fn next_frame(&mut self) -> Result<Option<Nv12Frame>>;
    /// Seek back to the start.
    fn restart(&mut self) -> Result<()>;
}

/// Opens `path` with the platform decoder.
pub fn open(path: &Path) -> Result<Box<dyn VideoDecoder>> {
    #[cfg(windows)]
    {
        windows::MfDecoder::open(path).map(|d| Box::new(d) as Box<dyn VideoDecoder>)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(Error::Unsupported)
    }
}

/// Decodes on a background thread, a few frames ahead, looping forever.
/// Frame timestamps keep increasing across loops so the consumer can pace
/// against a single clock.
#[derive(Debug)]
pub struct VideoPlayer {
    info: VideoInfo,
    frames: Receiver<Nv12Frame>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// Decoded frames buffered ahead of presentation. Small: each 4K frame is ~12 MB.
const QUEUE_DEPTH: usize = 3;

impl VideoPlayer {
    pub fn spawn(path: &Path) -> Result<Self> {
        let (tx, rx) = sync_channel(QUEUE_DEPTH);
        // The decoder is opened on its own thread (COM/MF state is per thread);
        // the open result comes back here so errors are still reported synchronously.
        let (opened_tx, opened_rx) = sync_channel::<Result<VideoInfo>>(1);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let path = path.to_path_buf();
        let thread = std::thread::Builder::new()
            .name("video-decode".into())
            .spawn(move || match open(&path) {
                Ok(decoder) => {
                    let _ = opened_tx.send(Ok(decoder.info()));
                    decode_loop(decoder, &tx, &thread_stop);
                }
                Err(e) => {
                    let _ = opened_tx.send(Err(e));
                }
            })
            .map_err(|e| Error::Os {
                call: "spawn decode thread",
                message: e.to_string(),
            })?;
        let info = opened_rx.recv().map_err(|_| Error::Os {
            call: "video decode thread",
            message: "exited before opening the file".into(),
        })??;
        Ok(Self {
            info,
            frames: rx,
            stop,
            thread: Some(thread),
        })
    }

    pub fn info(&self) -> VideoInfo {
        self.info
    }

    /// Next decoded frame if one is ready; never blocks.
    pub fn try_next(&self) -> Option<Nv12Frame> {
        self.frames.try_recv().ok()
    }

    /// Next decoded frame, waiting up to `timeout`.
    pub fn next_timeout(&self, timeout: Duration) -> Option<Nv12Frame> {
        self.frames.recv_timeout(timeout).ok()
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Unblock a sender waiting on a full queue.
        while self.frames.try_recv().is_ok() {}
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn decode_loop(mut decoder: Box<dyn VideoDecoder>, tx: &SyncSender<Nv12Frame>, stop: &AtomicBool) {
    let mut loop_offset = Duration::ZERO;
    let mut last_pts = Duration::ZERO;
    let frame_period = decoder
        .info()
        .frame_rate
        .filter(|r| *r > 0.0)
        .map_or(Duration::from_millis(33), |r| {
            Duration::from_secs_f64(1.0 / r)
        });

    while !stop.load(Ordering::SeqCst) {
        match decoder.next_frame() {
            Ok(Some(mut frame)) => {
                last_pts = frame.pts;
                frame.pts += loop_offset;
                let mut pending = frame;
                // Bounded send that still notices `stop` while the queue is full.
                loop {
                    match tx.try_send(pending) {
                        Ok(()) => break,
                        Err(TrySendError::Full(f)) => {
                            if stop.load(Ordering::SeqCst) {
                                return;
                            }
                            pending = f;
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(TrySendError::Disconnected(_)) => return,
                    }
                }
            }
            Ok(None) => {
                // Loop: the next pass starts one frame after the last frame of this one.
                loop_offset += last_pts + frame_period;
                if let Err(e) = decoder.restart() {
                    tracing::error!("video restart failed: {e}");
                    return;
                }
            }
            Err(e) => {
                tracing::error!("video decode failed: {e}");
                return;
            }
        }
    }
}
