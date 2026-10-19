//! Media backends (L3). Like `lumen-platform`, this crate may contain
//! OS-specific code (`scripts/check-layers.sh` allows it); everything above it
//! sees only the OS-neutral types here.
//!
//! Video: decoders produce NV12 frames (luma plane + interleaved chroma at half
//! resolution), the native output of hardware decoders. The GPU converts to RGB.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::JoinHandle;
use std::time::Duration;

// Only the Windows audio backend resamples so far; the tests run everywhere.
#[cfg_attr(not(windows), allow(dead_code))]
mod resample;
pub mod spectrum;
#[cfg(windows)]
mod windows;

#[cfg(not(windows))]
pub use unsupported_audio::{AudioCapture, AudioPlayer};
#[cfg(windows)]
pub use windows::audio::AudioPlayer;
#[cfg(windows)]
pub use windows::capture::AudioCapture;

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
    #[error("no audio output device")]
    NoAudioDevice,
    #[error("audio: {0}")]
    Audio(String),
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
    /// How many times the video has looped before this frame (to resync audio).
    pub loop_index: u32,
}

impl Nv12Frame {
    pub fn luma(&self) -> &[u8] {
        &self.data[..self.chroma_offset.min(self.data.len())]
    }

    pub fn chroma(&self) -> &[u8] {
        &self.data[self.chroma_offset.min(self.data.len())..]
    }

    /// CPU conversion to tightly packed RGBA8 (BT.709 limited range, same as the
    /// GPU shader). For snapshots, not per-frame playback.
    pub fn to_rgba(&self) -> Vec<u8> {
        let (w, h, stride) = (
            self.width as usize,
            self.height as usize,
            self.stride as usize,
        );
        let (luma, chroma) = (self.luma(), self.chroma());
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let yy = f32::from(luma.get(y * stride + x).copied().unwrap_or(16));
                let ci = (y / 2) * stride + (x / 2) * 2;
                let cb = f32::from(chroma.get(ci).copied().unwrap_or(128));
                let cr = f32::from(chroma.get(ci + 1).copied().unwrap_or(128));
                let yn = (yy - 16.0) * (255.0 / 219.0);
                let (u, v) = (
                    (cb - 128.0) * (255.0 / 224.0),
                    (cr - 128.0) * (255.0 / 224.0),
                );
                let rgb = [
                    yn + 1.5748 * v,
                    yn - 0.1873 * u - 0.4681 * v,
                    yn + 1.8556 * u,
                ];
                let o = (y * w + x) * 4;
                for (i, c) in rgb.iter().enumerate() {
                    out[o + i] = c.round().clamp(0.0, 255.0) as u8;
                }
                out[o + 3] = 255;
            }
        }
        out
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

/// Where a video or sound comes from.
#[derive(Debug, Clone)]
pub enum MediaSource {
    File(PathBuf),
    /// A whole media file in memory (e.g. inside a package); `name` is its
    /// file name, whose extension tells the decoder the container.
    Memory {
        data: Arc<[u8]>,
        name: String,
    },
}

impl MediaSource {
    /// File name, for logs.
    pub fn name(&self) -> String {
        match self {
            Self::File(p) => p.display().to_string(),
            Self::Memory { name, .. } => name.clone(),
        }
    }

    /// MIME type from the file extension, for decoders that need a hint.
    pub fn mime(&self) -> Option<&'static str> {
        let name = match self {
            Self::File(p) => p.to_string_lossy().into_owned(),
            Self::Memory { name, .. } => name.clone(),
        };
        let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
        Some(match ext.as_str() {
            "mp4" | "m4v" => "video/mp4",
            "webm" => "video/webm",
            "mp3" => "audio/mpeg",
            "wav" => "audio/wav",
            "m4a" | "aac" => "audio/mp4",
            "ogg" => "audio/ogg",
            "flac" => "audio/flac",
            _ => return None,
        })
    }
}

/// How a sound list plays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlaybackMode {
    /// Tracks in order, forever.
    Loop,
    /// Tracks in order, once.
    Once,
    /// A random track, then silence of `min..max` seconds, forever.
    Random { min_gap: f32, max_gap: f32 },
}

impl PlaybackMode {
    /// The track after `current` of `count` and the silence before it, or
    /// `None` when playback ends. `r1`, `r2`: uniform random numbers in 0..1.
    pub fn next_track(
        self,
        current: usize,
        count: usize,
        r1: f32,
        r2: f32,
    ) -> Option<(usize, f32)> {
        if count == 0 {
            return None;
        }
        match self {
            Self::Loop => Some(((current + 1) % count, 0.0)),
            Self::Once => (current + 1 < count).then_some((current + 1, 0.0)),
            Self::Random { min_gap, max_gap } => {
                // Another track than the one that just ended, when there is one.
                let next = if count > 1 {
                    let skip = ((r1 * (count - 1) as f32) as usize).min(count - 2);
                    (current + 1 + skip) % count
                } else {
                    0
                };
                let (lo, hi) = (min_gap.max(0.0), max_gap.max(min_gap).max(0.0));
                Some((next, lo + r2.clamp(0.0, 1.0) * (hi - lo)))
            }
        }
    }
}

/// Opens `path` with the platform decoder.
pub fn open(path: &Path) -> Result<Box<dyn VideoDecoder>> {
    open_source(&MediaSource::File(path.to_path_buf()))
}

/// Opens `source` with the platform decoder.
pub fn open_source(source: &MediaSource) -> Result<Box<dyn VideoDecoder>> {
    #[cfg(windows)]
    {
        windows::MfDecoder::open(source).map(|d| Box::new(d) as Box<dyn VideoDecoder>)
    }
    #[cfg(not(windows))]
    {
        let _ = source;
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
        Self::spawn_source(MediaSource::File(path.to_path_buf()))
    }

    pub fn spawn_source(source: MediaSource) -> Result<Self> {
        let (tx, rx) = sync_channel(QUEUE_DEPTH);
        // The decoder is opened on its own thread (COM/MF state is per thread);
        // the open result comes back here so errors are still reported synchronously.
        let (opened_tx, opened_rx) = sync_channel::<Result<VideoInfo>>(1);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("video-decode".into())
            .spawn(move || match open_source(&source) {
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
        if let Some(t) = self.thread.take() {
            // The decode thread may be blocked sending into a full queue: keep draining
            // until it notices `stop` and exits.
            while !t.is_finished() {
                while self.frames.try_recv().is_ok() {}
                std::thread::sleep(Duration::from_millis(1));
            }
            let _ = t.join();
        }
    }
}

fn decode_loop(mut decoder: Box<dyn VideoDecoder>, tx: &SyncSender<Nv12Frame>, stop: &AtomicBool) {
    let mut loop_offset = Duration::ZERO;
    let mut loop_index = 0u32;
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
                frame.loop_index = loop_index;
                // Blocks while the queue is full (e.g. playback paused): no CPU spent.
                // `VideoPlayer::drop` drains the queue so this returns on shutdown.
                if tx.send(frame).is_err() {
                    return;
                }
            }
            Ok(None) => {
                // Loop: the next pass starts one frame after the last frame of this one.
                loop_offset += last_pts + frame_period;
                loop_index = loop_index.wrapping_add(1);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(y: u8, cb: u8, cr: u8) -> Nv12Frame {
        // 2×2 frame: 4 luma bytes + one interleaved CbCr pair (stride 2).
        Nv12Frame {
            width: 2,
            height: 2,
            stride: 2,
            data: vec![y, y, y, y, cb, cr],
            chroma_offset: 4,
            pts: Duration::ZERO,
            loop_index: 0,
        }
    }

    #[test]
    fn playlists_loop_end_or_pick_other_tracks() {
        assert_eq!(
            PlaybackMode::Loop.next_track(2, 3, 0.5, 0.5),
            Some((0, 0.0))
        );
        assert_eq!(
            PlaybackMode::Once.next_track(0, 2, 0.5, 0.5),
            Some((1, 0.0))
        );
        assert_eq!(PlaybackMode::Once.next_track(1, 2, 0.5, 0.5), None);
        let random = PlaybackMode::Random {
            min_gap: 1.0,
            max_gap: 5.0,
        };
        for r in [0.0, 0.3, 0.99, 1.0] {
            let (next, gap) = random.next_track(1, 3, r, r).unwrap();
            assert_ne!(next, 1);
            assert!((1.0..=5.0).contains(&gap));
        }
        assert_eq!(random.next_track(0, 1, 0.7, 0.0), Some((0, 1.0)));
        assert_eq!(PlaybackMode::Loop.next_track(0, 0, 0.0, 0.0), None);
    }

    #[test]
    fn bt709_limited_range_extremes() {
        assert_eq!(&frame(16, 128, 128).to_rgba()[..4], &[0, 0, 0, 255]);
        assert_eq!(&frame(235, 128, 128).to_rgba()[..4], &[255, 255, 255, 255]);
        // Pure red in BT.709 limited range ≈ (63, 102, 240).
        let red = frame(63, 102, 240).to_rgba();
        assert!(red[0] > 240 && red[1] < 15 && red[2] < 15, "{red:?}");
    }
}

#[cfg(not(windows))]
mod unsupported_audio {
    use std::path::Path;

    use crate::{Error, Result};

    /// Uninhabited until a non-Windows audio backend exists.
    #[derive(Debug)]
    pub enum AudioPlayer {}

    /// Uninhabited until a non-Windows capture backend exists.
    #[derive(Debug)]
    pub enum AudioCapture {}

    impl AudioCapture {
        pub fn start() -> Result<Self> {
            Err(Error::Unsupported)
        }
        pub fn spectrum(&self, _dt: f32) -> crate::spectrum::AudioSpectrum {
            match *self {}
        }
        pub fn needs_reopen(&self) -> bool {
            match *self {}
        }
    }

    impl AudioPlayer {
        pub fn spawn(_path: &Path, _volume: f32) -> Result<Self> {
            Err(Error::Unsupported)
        }
        pub fn spawn_playlist(
            _tracks: Vec<crate::MediaSource>,
            _mode: crate::PlaybackMode,
            _volume: f32,
        ) -> Result<Self> {
            Err(Error::Unsupported)
        }
        pub fn is_playing(&self) -> bool {
            match *self {}
        }
        pub fn set_volume(&self, _volume: f32) {
            match *self {}
        }
        pub fn set_paused(&self, _paused: bool) {
            match *self {}
        }
        pub fn restart(&self) {
            match *self {}
        }
        pub fn needs_reopen(&self) -> bool {
            match *self {}
        }
    }
}
