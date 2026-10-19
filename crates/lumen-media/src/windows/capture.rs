//! What the system is playing, for audio-reactive wallpapers: a WASAPI
//! loopback stream on the default output device (cpal records an output
//! device when an input stream is built on it).

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
        let device = host.default_output_device().ok_or(Error::NoAudioDevice)?;
        let config = device.default_output_config().map_err(audio_err)?.config();
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
                move |e| {
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
            device: device.id().ok(),
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
