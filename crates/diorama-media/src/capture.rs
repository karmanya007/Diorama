//! What the system is playing, for audio-reactive wallpapers: on Windows a
//! WASAPI loopback stream on the default output device (cpal records an
//! output device when an input stream is built on it); on Linux the default
//! sink's monitor source (PulseAudio, or PipeWire's Pulse server).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::spectrum::{AudioSpectrum, SpectrumAnalyzer};
use crate::{Error, Result};

pub struct AudioCapture {
    analyzer: Arc<Mutex<SpectrumAnalyzer>>,
    failed: Arc<AtomicBool>,
    device: Option<cpal::DeviceId>,
    _stream: cpal::Stream,
}

impl std::fmt::Debug for AudioCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioCapture").finish_non_exhaustive()
    }
}

fn audio_err(e: impl std::fmt::Display) -> Error {
    Error::Audio(e.to_string())
}

impl AudioCapture {
    pub fn start() -> Result<Self> {
        let host = cpal::default_host();
        let output = host.default_output_device().ok_or(Error::NoAudioDevice)?;
        let (device, config) = loopback(&host, &output)?;
        let channels = usize::from(config.channels);
        let analyzer = Arc::new(Mutex::new(SpectrumAnalyzer::new(config.sample_rate)));
        let failed = Arc::new(AtomicBool::new(false));
        let (sink, err) = (Arc::clone(&analyzer), Arc::clone(&failed));
        let stream = device
            .build_input_stream::<f32, _, _>(
                config,
                move |data: &[f32], _| {
                    if let Ok(mut a) = sink.lock() {
                        a.push(data, channels);
                    }
                },
                move |e: cpal::Error| {
                    // An xrun (WASAPI glitch, common right after the stream
                    // starts) loses a few samples; reopening for it restarted
                    // capture every 2 s.
                    if e.kind() == cpal::ErrorKind::Xrun {
                        tracing::debug!("audio capture xrun");
                        return;
                    }
                    tracing::warn!("audio capture error: {e}");
                    err.store(true, Ordering::SeqCst);
                },
                None,
            )
            .map_err(audio_err)?;
        stream.play().map_err(audio_err)?;
        Ok(Self {
            analyzer,
            failed,
            device: output.id().ok(),
            _stream: stream,
        })
    }

    /// Band levels now; `dt` seconds since the previous call.
    pub fn spectrum(&self, dt: f32) -> AudioSpectrum {
        self.analyzer
            .lock()
            .map(|mut a| a.analyze(dt))
            .unwrap_or_default()
    }

    /// What WE hands web wallpapers now (linear, unsmoothed).
    pub fn web_spectrum(&self) -> AudioSpectrum {
        self.analyzer
            .lock()
            .map(|a| a.web_levels())
            .unwrap_or_default()
    }

    /// True if the stream failed or the default output changed (restart it).
    pub fn needs_reopen(&self) -> bool {
        if self.failed.load(Ordering::SeqCst) {
            return true;
        }
        cpal::default_host()
            .default_output_device()
            .and_then(|d| d.id().ok())
            != self.device
    }
}

/// The device that records what `output` plays, and its format.
#[cfg(windows)]
fn loopback(_: &cpal::Host, output: &cpal::Device) -> Result<(cpal::Device, cpal::StreamConfig)> {
    let config = output.default_output_config().map_err(audio_err)?.config();
    Ok((output.clone(), config))
}

/// The device that records what `output` plays, and its format: the sink's
/// monitor source (`<sink>.monitor`, PulseAudio's naming, which PipeWire
/// keeps). Plain ALSA has none.
#[cfg(not(windows))]
fn loopback(
    host: &cpal::Host,
    output: &cpal::Device,
) -> Result<(cpal::Device, cpal::StreamConfig)> {
    let monitor = format!("{}.monitor", output.id().map_err(audio_err)?.id());
    let device = host
        .input_devices()
        .map_err(audio_err)?
        .find(|d| d.id().is_ok_and(|id| id.id() == monitor))
        .ok_or_else(|| Error::Audio(format!("no monitor source ({monitor}) to record playback")))?;
    let config = device.default_input_config().map_err(audio_err)?.config();
    Ok((device, config))
}
