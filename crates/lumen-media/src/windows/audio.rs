//! Sound playback (video soundtracks, scene sound layers): Media Foundation
//! decodes the audio stream to float on a background thread; cpal (WASAPI
//! shared mode) plays it on the current default output device.
//!
//! Failure here never affects video (P10): no device, a lost device or an
//! unsupported codec just means silence plus a log line.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSourceReader, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READERF_ENDOFSTREAM, MFAudioFormat_Float,
    MFCreateAttributes, MFCreateMediaType, MFMediaType_Audio,
};

use super::{MfRuntime, os, source_reader};
use crate::resample::Resampler;
use crate::{Error, MediaSource, PlaybackMode, Result};

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
    /// A `PlaybackMode::Once` list reached its end.
    finished: AtomicBool,
}

impl Shared {
    fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }
}

/// Plays a sound list (or a video file's soundtrack). Create and drop on one
/// thread (cpal streams are not `Send` on every backend).
pub struct AudioPlayer {
    shared: Arc<Shared>,
    decoder: Option<JoinHandle<()>>,
    device: Option<cpal::DeviceId>,
    _stream: cpal::Stream,
    name: String,
}

impl std::fmt::Debug for AudioPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioPlayer")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

fn audio_err(e: impl std::fmt::Display) -> Error {
    Error::Audio(e.to_string())
}

impl AudioPlayer {
    /// A video's soundtrack, looping. Starts paused; call
    /// [`AudioPlayer::set_paused`]`(false)` when video starts.
    pub fn spawn(path: &Path, volume: f32) -> Result<Self> {
        Self::spawn_playlist(
            vec![MediaSource::File(path.to_path_buf())],
            PlaybackMode::Loop,
            volume,
        )
    }

    /// Plays `tracks` as `mode` says. Starts paused.
    pub fn spawn_playlist(
        tracks: Vec<MediaSource>,
        mode: PlaybackMode,
        volume: f32,
    ) -> Result<Self> {
        if tracks.is_empty() {
            return Err(Error::Audio("no tracks".into()));
        }
        let name = tracks[0].name();
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
            finished: AtomicBool::new(false),
        });

        let decode_shared = Arc::clone(&shared);
        let decoder = std::thread::Builder::new()
            .name("audio-decode".into())
            .spawn(move || {
                let out = Output {
                    rate: dst_rate,
                    channels: dst_channels,
                };
                if let Err(e) = decode_loop(&tracks, mode, out, &decode_shared) {
                    tracing::warn!("audio disabled: {e}");
                    decode_shared.finished.store(true, Ordering::SeqCst);
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
            name,
        })
    }

    /// Playing: not paused and not at the end of a `Once` list.
    pub fn is_playing(&self) -> bool {
        !self.shared.paused.load(Ordering::Relaxed) && !self.shared.finished.load(Ordering::SeqCst)
    }

    pub fn set_volume(&self, volume: f32) {
        self.shared
            .volume
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }

    /// Jump back to the start (the video looped, or a sound is replayed).
    pub fn restart(&self) {
        self.shared.finished.store(false, Ordering::SeqCst);
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

fn open_audio_reader(source: &MediaSource) -> Result<(IMFSourceReader, u32, usize)> {
    // SAFETY: COM calls on interfaces we own; GUID pointers reference constants.
    unsafe {
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 1).map_err(os("MFCreateAttributes"))?;
        let attrs = attrs.ok_or_else(|| Error::Audio("no attributes".into()))?;
        let reader = source_reader(source, &attrs)?;
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .map_err(os("SetStreamSelection(all)"))?;
        reader
            .SetStreamSelection(AUDIO_STREAM, true)
            .map_err(|_| Error::Audio(format!("{} has no audio track", source.name())))?;
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

/// The device's format.
#[derive(Clone, Copy)]
struct Output {
    rate: u32,
    channels: usize,
}

/// One track being decoded.
struct Track {
    index: usize,
    reader: IMFSourceReader,
    resampler: Resampler,
}

impl Track {
    fn open(tracks: &[MediaSource], index: usize, out: Output) -> Result<Self> {
        let source = &tracks[index];
        let (reader, rate, channels) = open_audio_reader(source)?;
        tracing::info!(
            track = %source.name(),
            rate,
            channels,
            out_rate = out.rate,
            out_channels = out.channels,
            "audio opened"
        );
        Ok(Self {
            index,
            reader,
            resampler: Resampler::new(rate, channels, out.rate, out.channels),
        })
    }
}

/// Small xorshift for picking random tracks and gaps.
struct Rng(u64);

impl Rng {
    fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9E37_79B9, |d| d.as_nanos() as u64);
        Self(seed | 1)
    }

    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

fn first_track(mode: PlaybackMode, count: usize, rng: &mut Rng) -> usize {
    match mode {
        PlaybackMode::Random { .. } => ((rng.next() * count as f32) as usize).min(count - 1),
        _ => 0,
    }
}

fn decode_loop(
    tracks: &[MediaSource],
    mode: PlaybackMode,
    out: Output,
    shared: &Shared,
) -> Result<()> {
    let _runtime = MfRuntime::start()?;
    let mut rng = Rng::new();
    let mut track = Track::open(tracks, first_track(mode, tracks.len(), &mut rng), out)?;
    let mut buf = Vec::new();

    while !shared.stop.load(Ordering::SeqCst) {
        if shared.restart.swap(false, Ordering::SeqCst) {
            track = Track::open(tracks, first_track(mode, tracks.len(), &mut rng), out)?;
            if let Ok(mut s) = shared.samples.lock() {
                s.clear();
            }
        }
        if shared.finished.load(Ordering::SeqCst) {
            // Ended: wait for a restart (or shutdown).
            if let Ok(s) = shared.samples.lock() {
                let _ = shared.space.wait_timeout(s, Duration::from_millis(50));
            }
            continue;
        }

        let mut flags = 0u32;
        let mut sample = None;
        // SAFETY: out-params are valid locals.
        unsafe {
            track
                .reader
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
            let (r1, r2) = (rng.next(), rng.next());
            match mode.next_track(track.index, tracks.len(), r1, r2) {
                Some((next, gap)) => {
                    // Silence between tracks goes through the queue like sound.
                    let silence = (gap * out.rate as f32) as usize * out.channels;
                    for _ in 0..silence.div_ceil(4096) {
                        buf.clear();
                        buf.resize(4096, 0.0);
                        if !push(shared, &buf) {
                            break;
                        }
                    }
                    track = Track::open(tracks, next, out)?;
                }
                None => shared.finished.store(true, Ordering::SeqCst),
            }
            continue;
        }
        let Some(sample) = sample else { continue };

        buf.clear();
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
            track.resampler.process(floats, &mut buf);
            buffer
                .Unlock()
                .map_err(os("IMFMediaBuffer::Unlock(audio)"))?;
        }
        push(shared, &buf);
    }
    Ok(())
}

/// Queues `samples` once there is room. False when stopping or restarting
/// (the samples are dropped).
fn push(shared: &Shared, samples: &[f32]) -> bool {
    let Ok(mut queue) = shared.samples.lock() else {
        return false;
    };
    // Wait for room; the device callback frees space and notifies.
    while queue.len() + samples.len() > shared.capacity
        && !shared.stop.load(Ordering::SeqCst)
        && !shared.restart.load(Ordering::SeqCst)
    {
        queue = match shared.space.wait_timeout(queue, Duration::from_millis(50)) {
            Ok((q, _)) => q,
            Err(_) => return false,
        };
    }
    if shared.stop.load(Ordering::SeqCst) || shared.restart.load(Ordering::SeqCst) {
        return false;
    }
    queue.extend(samples.iter().copied());
    true
}
