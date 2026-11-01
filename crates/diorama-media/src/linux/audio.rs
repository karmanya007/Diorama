//! GStreamer track reader for sound playback: decodes a file's first audio
//! stream to interleaved 32-bit float.

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;

use super::{Media, bus_error, decode_pipeline, init};
use crate::{Error, MediaSource, Result};

/// How long opening a track or decoding a chunk of it may take.
const TIMEOUT: gst::ClockTime = gst::ClockTime::from_seconds(10);

/// GStreamer needs nothing per thread.
pub(crate) fn decode_thread() -> Result<impl Sized> {
    init()
}

pub(crate) struct TrackReader {
    pipeline: gst::Pipeline,
    sink: gst_app::AppSink,
    rate: u32,
    channels: usize,
}

impl TrackReader {
    pub(crate) fn open(source: &MediaSource) -> Result<Self> {
        init()?;
        let sink = gst_app::AppSink::builder()
            .caps(
                &gst::Caps::builder("audio/x-raw")
                    .field("format", "F32LE")
                    .field("layout", "interleaved")
                    .build(),
            )
            .sync(false)
            .max_buffers(4)
            .build();
        let (pipeline, sink, _) = decode_pipeline(source, Media::Audio, sink)?;
        let mut reader = Self {
            pipeline,
            sink,
            rate: 0,
            channels: 0,
        };
        let failed = |pipeline: &gst::Pipeline| {
            let why =
                bus_error(pipeline).map_or_else(|| "no audio track".into(), |e| e.to_string());
            Error::Audio(format!("{}: {why}", source.name()))
        };
        if reader.pipeline.set_state(gst::State::Paused).is_err() {
            return Err(failed(&reader.pipeline));
        }
        // The prerolled sample also comes out of the first `read`.
        let Some(sample) = reader.sink.try_pull_preroll(TIMEOUT) else {
            return Err(failed(&reader.pipeline));
        };
        let s = sample.caps().and_then(|c| c.structure(0));
        let field = |name| s.and_then(|s| s.get::<i32>(name).ok()).unwrap_or(0).max(0);
        let (rate, channels) = (field("rate") as u32, field("channels") as usize);
        if rate == 0 || channels == 0 {
            return Err(Error::Audio(format!("{}: no sample format", source.name())));
        }
        if reader.pipeline.set_state(gst::State::Playing).is_err() {
            return Err(failed(&reader.pipeline));
        }
        (reader.rate, reader.channels) = (rate, channels);
        Ok(reader)
    }

    /// Sample rate and channel count of what [`Self::read`] appends.
    pub(crate) fn format(&self) -> (u32, usize) {
        (self.rate, self.channels)
    }

    /// Appends the next decoded samples (interleaved) to `out`; `false` at
    /// the end of the track.
    pub(crate) fn read(&mut self, out: &mut Vec<f32>) -> Result<bool> {
        let Some(sample) = self.sink.try_pull_sample(TIMEOUT) else {
            if self.sink.is_eos() {
                return Ok(false);
            }
            return Err(bus_error(&self.pipeline)
                .unwrap_or_else(|| Error::Audio("sound decoder produced nothing in time".into())));
        };
        let Some(map) = sample.buffer().and_then(|b| b.map_readable().ok()) else {
            return Ok(true);
        };
        out.extend(
            map.as_slice()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b)),
        );
        Ok(true)
    }
}

impl Drop for TrackReader {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}
