//! Video soundtrack playback: Media Foundation decodes the file's audio stream
//! to float on a background thread; cpal (WASAPI shared mode) plays it on the
//! current default output device.
//!
//! Failure here never affects video (P10): no device, a lost device or an
//! unsupported codec just means silence plus a log line.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSourceReader, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READERF_ENDOFSTREAM, MFAudioFormat_Float,
    MFCreateAttributes, MFCreateMediaType, MFCreateSourceReaderFromURL, MFMediaType_Audio,
};
use windows::core::HSTRING;

use super::{MfRuntime, os};
use crate::resample::Resampler;
use crate::{Error, Result};

const AUDIO_STREAM: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
/// Decoded audio buffered ahead of the device (seconds).
const BUFFER_SECONDS: f32 = 0.5;

struct Shared {
    samples: Mutex<VecDeque<f32>>,
    space: Condvar,
    capacity: usize,
    volume: AtomicU32,
    paused: AtomicBool,
    stop: AtomicBool,
    restart: AtomicBool,
    stream_failed: AtomicBool,
}

impl Shared {
    fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }
}

/// Plays a video file's soundtrack. Create and drop on one thread (cpal streams
/// are not `Send` on every backend).
pub struct AudioPlayer {
    shared: Arc<Shared>,
    decoder: Option<JoinHandle<()>>,
    device: Option<cpal::DeviceId>,
    _stream: cpal::Stream,
    path: PathBuf,
}

impl std::fmt::Debug for AudioPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioPlayer")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

fn audio_err(e: impl std::fmt::Display) -> Error {
    Error::Audio(e.to_string())
}

impl AudioPlayer {
    /// Starts paused; call [`AudioPlayer::set_paused`]`(false)` when video starts.
    pub fn spawn(path: &Path, volume: f32) -> Result<Self> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or(Error::NoAudioDevice)?;
        let config = device.default_output_config().map_err(audio_err)?.config();
        let (dst_rate, dst_channels) = (config.sample_rate, usize::from(config.channels));

        let shared = Arc::new(Shared {
            samples: Mutex::new(VecDeque::new()),
            space: Condvar::new(),
            capacity: (dst_rate as f32 * BUFFER_SECONDS) as usize * dst_channels,
            volume: AtomicU32::new(volume.clamp(0.0, 1.0).to_bits()),
            paused: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            restart: AtomicBool::new(false),
            stream_failed: AtomicBool::new(false),
        });

        let decode_shared = Arc::clone(&shared);
        let decode_path = path.to_path_buf();
        let decoder = std::thread::Builder::new()
            .name("audio-decode".into())
            .spawn(move || {
                if let Err(e) = decode_loop(&decode_path, dst_rate, dst_channels, &decode_shared) {
                    tracing::warn!("video audio disabled: {e}");
                }
            })
            .map_err(audio_err)?;

        let cb = Arc::clone(&shared);
        let err = Arc::clone(&shared);
        let stream = device
            .build_output_stream::<f32, _, _>(
                config,
                move |out: &mut [f32], _| fill(&cb, out),
                move |e| {
                    // Device unplugged / default changed / exclusive mode grabbed: rebuild later.
                    tracing::warn!("audio stream error: {e}");
                    err.stream_failed.store(true, Ordering::SeqCst);
                },
                None,
            )
            .map_err(audio_err)?;
        stream.play().map_err(audio_err)?;

        Ok(Self {
            shared,
            decoder: Some(decoder),
            device: device.id().ok(),
            _stream: stream,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn set_volume(&self, volume: f32) {
        self.shared
            .volume
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }

    /// Jump back to the start (the video looped).
    pub fn restart(&self) {
        self.shared.restart.store(true, Ordering::SeqCst);
        self.shared.space.notify_all();
    }

    /// True if the stream failed or the system default device changed; the
    /// caller should drop this player and spawn a new one.
    pub fn needs_reopen(&self) -> bool {
        if self.shared.stream_failed.load(Ordering::SeqCst) {
            return true;
        }
        let current = cpal::default_host()
            .default_output_device()
            .and_then(|d| d.id().ok());
        current != self.device
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.space.notify_all();
        if let Some(t) = self.decoder.take() {
            let _ = t.join();
        }
    }
}

/// Device callback: never blocks for long, outputs silence on underrun or pause.
fn fill(shared: &Shared, out: &mut [f32]) {
    if shared.paused.load(Ordering::Relaxed) {
        out.fill(0.0);
        return;
    }
    let volume = shared.volume();
    let Ok(mut samples) = shared.samples.lock() else {
        out.fill(0.0);
        return;
    };
    for s in out.iter_mut() {
        *s = samples.pop_front().unwrap_or(0.0) * volume;
    }
    drop(samples);
    shared.space.notify_one();
}

fn open_audio_reader(path: &HSTRING) -> Result<(IMFSourceReader, u32, usize)> {
    // SAFETY: COM calls on interfaces we own; GUID pointers reference constants.
    unsafe {
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 1).map_err(os("MFCreateAttributes"))?;
        let attrs = attrs.ok_or_else(|| Error::Audio("no attributes".into()))?;
        let reader = MFCreateSourceReaderFromURL(path, &attrs)
            .map_err(os("MFCreateSourceReaderFromURL(audio)"))?;
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .map_err(os("SetStreamSelection(all)"))?;
        reader
            .SetStreamSelection(AUDIO_STREAM, true)
            .map_err(|_| Error::Audio("file has no audio track".into()))?;
        let wanted = MFCreateMediaType().map_err(os("MFCreateMediaType"))?;
        wanted
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(os("SetGUID(major)"))?;
        wanted
            .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)
            .map_err(os("SetGUID(float)"))?;
        reader
            .SetCurrentMediaType(AUDIO_STREAM, None, &wanted)
            .map_err(os("SetCurrentMediaType(float)"))?;
        let current = reader
            .GetCurrentMediaType(AUDIO_STREAM)
            .map_err(os("GetCurrentMediaType(audio)"))?;
        let rate = current
            .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
            .map_err(os("MF_MT_AUDIO_SAMPLES_PER_SECOND"))?;
        let channels = current
            .GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)
            .map_err(os("MF_MT_AUDIO_NUM_CHANNELS"))? as usize;
        Ok((reader, rate, channels))
    }
}

fn decode_loop(path: &Path, dst_rate: u32, dst_channels: usize, shared: &Shared) -> Result<()> {
    let _runtime = MfRuntime::start()?;
    let path = HSTRING::from(path.as_os_str());
    let (mut reader, src_rate, src_channels) = open_audio_reader(&path)?;
    let mut resampler = Resampler::new(src_rate, src_channels, dst_rate, dst_channels);
    tracing::info!(
        src_rate,
        src_channels,
        dst_rate,
        dst_channels,
        "video audio opened"
    );
    let mut out = Vec::new();

    while !shared.stop.load(Ordering::SeqCst) {
        if shared.restart.swap(false, Ordering::SeqCst) {
            (reader, _, _) = open_audio_reader(&path)?;
            resampler.reset();
            if let Ok(mut s) = shared.samples.lock() {
                s.clear();
            }
        }

        let mut flags = 0u32;
        let mut sample = None;
        // SAFETY: out-params are valid locals.
        unsafe {
            reader
                .ReadSample(
                    AUDIO_STREAM,
                    0,
                    None,
                    Some(&mut flags),
                    None,
                    Some(&mut sample),
                )
                .map_err(os("ReadSample(audio)"))?;
        }
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            // Loop on our own too, in case the video loop signal comes late.
            (reader, _, _) = open_audio_reader(&path)?;
            resampler.reset();
            continue;
        }
        let Some(sample) = sample else { continue };

        out.clear();
        // SAFETY: buffer locked only for the duration of the copy.
        unsafe {
            let buffer = sample
                .ConvertToContiguousBuffer()
                .map_err(os("ConvertToContiguousBuffer(audio)"))?;
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            buffer
                .Lock(&mut ptr, None, Some(&mut len))
                .map_err(os("IMFMediaBuffer::Lock(audio)"))?;
            let floats = std::slice::from_raw_parts(ptr.cast::<f32>(), len as usize / 4);
            resampler.process(floats, &mut out);
            buffer
                .Unlock()
                .map_err(os("IMFMediaBuffer::Unlock(audio)"))?;
        }

        // Wait for room; the device callback frees space and notifies.
        let Ok(mut samples) = shared.samples.lock() else {
            return Ok(());
        };
        while samples.len() + out.len() > shared.capacity
            && !shared.stop.load(Ordering::SeqCst)
            && !shared.restart.load(Ordering::SeqCst)
        {
            samples = match shared
                .space
                .wait_timeout(samples, Duration::from_millis(50))
            {
                Ok((s, _)) => s,
                Err(_) => return Ok(()),
            };
        }
        if !shared.restart.load(Ordering::SeqCst) {
            samples.extend(out.iter().copied());
        }
    }
    Ok(())
}
