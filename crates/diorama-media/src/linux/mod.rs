//! GStreamer decoders (Linux): video here, sound tracks in [`audio`].
//!
//! `decodebin` picks the decoder by rank: the GPU's (VA-API, NVDEC, V4L2)
//! when its plugin is installed, else libav on the CPU. Only the video stream
//! is linked, so playback never depends on an audio device (P10).

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use gstreamer_video::prelude::*;

use crate::{Error, MediaSource, Nv12Frame, Result, VideoDecoder, VideoInfo, VideoProbe};

pub(crate) mod audio;

/// How long opening may take before the file counts as undecodable, and how
/// long one frame may take (4K on a slow CPU stays well below).
const OPEN_TIMEOUT: gst::ClockTime = gst::ClockTime::from_seconds(10);
const FRAME_TIMEOUT: gst::ClockTime = gst::ClockTime::from_seconds(10);

pub(super) fn gst_err(call: &'static str) -> impl FnOnce(gst::glib::BoolError) -> Error {
    move |e| Error::Os {
        call,
        message: e.to_string(),
    }
}

pub(super) fn init() -> Result<()> {
    static INIT: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| gst::init().map_err(|e| e.to_string()))
        .clone()
        .map_err(|message| Error::Os {
            call: "gst_init",
            message,
        })
}

/// What `decodebin` chose (or failed to find) for the stream.
#[derive(Debug, Default)]
pub(super) struct Choice {
    decoder: Option<gst::Element>,
    /// The decoder's class says it runs on dedicated hardware.
    hardware: bool,
    /// Caps of a stream no installed plugin decodes.
    unknown: Option<gst::Caps>,
}

#[derive(Debug)]
pub(crate) struct GstDecoder {
    source: MediaSource,
    pipeline: gst::Pipeline,
    sink: gst_app::AppSink,
    info: VideoInfo,
    choice: Arc<Mutex<Choice>>,
}

impl GstDecoder {
    pub(crate) fn open(source: &MediaSource) -> Result<Self> {
        init()?;
        let (pipeline, sink, choice) = build(source)?;
        let preroll = preroll(&pipeline, &sink);
        let info = match preroll {
            Ok(info) => info,
            Err(e) => {
                let _ = pipeline.set_state(gst::State::Null);
                return Err(e);
            }
        };
        if let Some(d) = lock(&choice).decoder.as_ref().and_then(|d| d.factory()) {
            tracing::debug!(decoder = %d.name(), "GStreamer video decoder");
        }
        Ok(Self {
            source: source.clone(),
            pipeline,
            sink,
            info,
            choice,
        })
    }
}

impl Drop for GstDecoder {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

impl VideoDecoder for GstDecoder {
    fn info(&self) -> VideoInfo {
        self.info
    }

    fn next_frame(&mut self) -> Result<Option<Nv12Frame>> {
        match self.sink.try_pull_sample(FRAME_TIMEOUT) {
            Some(sample) => nv12_from_sample(&sample).map(Some),
            None if self.sink.is_eos() => Ok(None),
            None => Err(bus_error(&self.pipeline)
                .unwrap_or_else(|| Error::BadFrame("decoder produced no frame in time".into()))),
        }
    }

    fn restart(&mut self) -> Result<()> {
        // Seeking back would work for files, but a fresh pipeline is as fast
        // and behaves the same for every source.
        let _ = self.pipeline.set_state(gst::State::Null);
        let (pipeline, sink, choice) = build(&self.source)?;
        self.pipeline = pipeline;
        self.sink = sink;
        self.choice = choice;
        self.info = preroll(&self.pipeline, &self.sink)?;
        Ok(())
    }
}

pub(super) fn lock(choice: &Mutex<Choice>) -> std::sync::MutexGuard<'_, Choice> {
    choice
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

type Decoding = (gst::Pipeline, gst_app::AppSink, Arc<Mutex<Choice>>);

/// `source ! decodebin ! videoconvert ! appsink(NV12)`.
fn build(source: &MediaSource) -> Result<Decoding> {
    let sink = gst_app::AppSink::builder()
        .caps(
            &gst_video::VideoCapsBuilder::new()
                .format(gst_video::VideoFormat::Nv12)
                .build(),
        )
        // Frames are paced by the player; decode as fast as the queue drains.
        .sync(false)
        .max_buffers(2)
        .build();
    decode_pipeline(source, Media::Video, sink)
}

/// The kind of stream a pipeline decodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Media {
    Video,
    Audio,
}

impl Media {
    fn caps_prefix(self) -> &'static str {
        match self {
            Self::Video => "video/",
            Self::Audio => "audio/",
        }
    }

    /// The word in a decoder's class (`Codec/Decoder/Video`).
    fn klass(self) -> &'static str {
        match self {
            Self::Video => "Video",
            Self::Audio => "Audio",
        }
    }

    fn converter(self) -> &'static str {
        match self {
            Self::Video => "videoconvert",
            Self::Audio => "audioconvert",
        }
    }
}

/// `source ! decodebin ! (video|audio)convert ! sink` with only the first
/// stream of `media` linked; the others stay unlinked and are dropped.
pub(super) fn decode_pipeline(
    source: &MediaSource,
    media: Media,
    sink: gst_app::AppSink,
) -> Result<Decoding> {
    let pipeline = gst::Pipeline::new();
    let src = match source {
        MediaSource::File(path) => gst::ElementFactory::make("filesrc")
            .property("location", path.to_string_lossy().as_ref())
            .build()
            .map_err(gst_err("filesrc"))?,
        MediaSource::Memory { data, .. } => memory_source(Arc::clone(data)).upcast(),
    };
    let decodebin = gst::ElementFactory::make("decodebin")
        .build()
        .map_err(gst_err("decodebin"))?;
    let convert = gst::ElementFactory::make(media.converter())
        .build()
        .map_err(gst_err(media.converter()))?;
    pipeline
        .add_many([&src, &decodebin, &convert, sink.upcast_ref()])
        .map_err(gst_err("gst_bin_add"))?;
    src.link(&decodebin).map_err(gst_err("link source"))?;
    convert.link(&sink).map_err(gst_err("link appsink"))?;

    let convert_sink = convert
        .static_pad("sink")
        .ok_or(Error::BadFrame("converter without a sink pad".into()))?;
    decodebin.connect_pad_added(move |_, pad| {
        let wanted = pad
            .current_caps()
            .or_else(|| Some(pad.query_caps(None)))
            .and_then(|c| {
                c.structure(0)
                    .map(|s| s.name().starts_with(media.caps_prefix()))
            })
            .unwrap_or(false);
        if wanted
            && !convert_sink.is_linked()
            && let Err(e) = pad.link(&convert_sink)
        {
            tracing::warn!("linking the decoded stream: {e}");
        }
    });

    let choice = Arc::new(Mutex::new(Choice::default()));
    let chosen = Arc::clone(&choice);
    decodebin
        .downcast_ref::<gst::Bin>()
        .ok_or(Error::BadFrame("decodebin is not a bin".into()))?
        .connect_deep_element_added(move |_, _, element| {
            let Some(klass) = element
                .factory()
                .and_then(|f| f.metadata(gst::ELEMENT_METADATA_KLASS).map(str::to_owned))
            else {
                return;
            };
            if klass.contains("Decoder") && klass.contains(media.klass()) {
                let mut c = lock(&chosen);
                c.hardware = klass.contains("Hardware");
                c.decoder = Some(element.clone());
            }
        });
    let unknown = Arc::clone(&choice);
    decodebin.connect("unknown-type", false, move |args| {
        if let Some(caps) = args.get(2).and_then(|v| v.get::<gst::Caps>().ok())
            && caps
                .structure(0)
                .is_some_and(|s| s.name().starts_with(media.caps_prefix()))
        {
            lock(&unknown).unknown = Some(caps);
        }
        None
    });
    Ok((pipeline, sink, choice))
}

/// A seekable source over a file held in memory (media inside packages).
fn memory_source(data: Arc<[u8]>) -> gst_app::AppSrc {
    let offset = Arc::new(Mutex::new(0usize));
    let seek_to = Arc::clone(&offset);
    let src = gst_app::AppSrc::builder()
        .stream_type(gst_app::AppStreamType::RandomAccess)
        .format(gst::Format::Bytes)
        .size(data.len() as i64)
        .build();
    src.set_callbacks(
        gst_app::AppSrcCallbacks::builder()
            .need_data(move |src, length| {
                let mut at = offset
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if *at >= data.len() {
                    let _ = src.end_of_stream();
                    return;
                }
                // Pull mode (qtdemux) needs exactly what it asked for; 0 or
                // u32::MAX means no particular size.
                let want = match length {
                    0 | u32::MAX => 64 << 10,
                    n => n as usize,
                };
                let end = (*at + want).min(data.len());
                let chunk = data[*at..end].to_vec();
                *at = end;
                let _ = src.push_buffer(gst::Buffer::from_mut_slice(chunk));
            })
            .seek_data(move |_, to| {
                *seek_to
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = to as usize;
                true
            })
            .build(),
    );
    src
}

/// Waits for the first frame and reads the video's size and rate from it.
fn preroll(pipeline: &gst::Pipeline, sink: &gst_app::AppSink) -> Result<VideoInfo> {
    pipeline
        .set_state(gst::State::Paused)
        .map_err(|_| bus_error(pipeline).unwrap_or(Error::Undecodable("unknown format".into())))?;
    let Some(sample) = sink.try_pull_preroll(OPEN_TIMEOUT) else {
        return Err(bus_error(pipeline).unwrap_or(Error::Undecodable("no video frame".into())));
    };
    let info = sample
        .caps()
        .and_then(|c| gst_video::VideoInfo::from_caps(c).ok())
        .ok_or(Error::BadFrame("frame without video caps".into()))?;
    let fps = info.fps();
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|_| bus_error(pipeline).unwrap_or(Error::BadFrame("cannot play".into())))?;
    Ok(VideoInfo {
        width: info.width(),
        height: info.height(),
        frame_rate: (fps.numer() > 0 && fps.denom() > 0)
            .then(|| f64::from(fps.numer()) / f64::from(fps.denom())),
    })
}

/// The first error the pipeline posted, if any.
pub(super) fn bus_error(pipeline: &gst::Pipeline) -> Option<Error> {
    let msg = pipeline.bus()?.pop_filtered(&[gst::MessageType::Error])?;
    let gst::MessageView::Error(err) = msg.view() else {
        return None;
    };
    Some(Error::Os {
        call: "GStreamer",
        message: match err.debug() {
            Some(debug) => format!("{} ({debug})", err.error()),
            None => err.error().to_string(),
        },
    })
}

/// Copies a decoded NV12 sample into one buffer with a common row stride.
fn nv12_from_sample(sample: &gst::Sample) -> Result<Nv12Frame> {
    let buffer = sample
        .buffer()
        .ok_or(Error::BadFrame("sample without a buffer".into()))?;
    let info = sample
        .caps()
        .and_then(|c| gst_video::VideoInfo::from_caps(c).ok())
        .ok_or(Error::BadFrame("sample without video caps".into()))?;
    let frame = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info)
        .map_err(|e| Error::BadFrame(e.to_string()))?;
    let (width, height) = (info.width(), info.height());
    let strides = frame.plane_stride();
    let stride = strides
        .first()
        .map_or(width as usize, |s| s.unsigned_abs() as usize)
        .max(width as usize);
    let chroma_rows = height.div_ceil(2) as usize;
    let mut data = vec![0u8; stride * (height as usize + chroma_rows)];
    for (plane, rows, out_start) in [
        (0, height as usize, 0),
        (1, chroma_rows, stride * height as usize),
    ] {
        let src = frame
            .plane_data(plane)
            .map_err(|e| Error::BadFrame(e.to_string()))?;
        let src_stride = strides
            .get(plane as usize)
            .map_or(stride, |s| s.unsigned_abs() as usize);
        for row in 0..rows {
            let from = row * src_stride;
            let n = src_stride.min(stride).min(src.len().saturating_sub(from));
            let to = out_start + row * stride;
            data[to..to + n].copy_from_slice(&src[from..from + n]);
        }
    }
    Ok(Nv12Frame {
        width,
        height,
        stride: stride as u32,
        chroma_offset: stride * height as usize,
        data,
        pts: buffer
            .pts()
            .map_or(Duration::ZERO, |t| Duration::from_nanos(t.nseconds())),
        loop_index: 0,
    })
}

/// Whether `path`'s video decodes here, and on the GPU (P4).
pub(crate) fn probe(path: &std::path::Path) -> Result<VideoProbe> {
    init()?;
    let (pipeline, sink, choice) = build(&MediaSource::File(path.to_path_buf()))?;
    let prerolled = preroll(&pipeline, &sink);
    let probe = {
        let c = lock(&choice);
        match prerolled {
            Ok(info) => Ok(VideoProbe {
                codec: c
                    .decoder
                    .as_ref()
                    .and_then(|d| d.static_pad("sink"))
                    .and_then(|p| p.current_caps())
                    .map_or_else(|| "unknown".into(), |caps| codec_name(&caps)),
                width: info.width,
                height: info.height,
                hardware: c.hardware,
                decodable: true,
            }),
            // A video stream no installed plugin decodes.
            Err(e) => match &c.unknown {
                Some(caps) => {
                    let s = caps.structure(0);
                    let side =
                        |name| s.and_then(|s| s.get::<i32>(name).ok()).unwrap_or(0).max(0) as u32;
                    Ok(VideoProbe {
                        codec: codec_name(caps),
                        width: side("width"),
                        height: side("height"),
                        hardware: false,
                        decodable: false,
                    })
                }
                None => Err(e),
            },
        }
    };
    let _ = pipeline.set_state(gst::State::Null);
    probe
}

/// `H.264`, `H.265 main-10`, … from a compressed stream's caps.
fn codec_name(caps: &gst::CapsRef) -> String {
    let Some(s) = caps.structure(0) else {
        return "unknown".into();
    };
    let name = match s.name().as_str() {
        "video/x-h264" => "H.264",
        "video/x-h265" => "H.265",
        "video/x-vp8" => "VP8",
        "video/x-vp9" => "VP9",
        "video/x-av1" => "AV1",
        "video/mpeg" if s.get::<i32>("mpegversion") == Ok(4) => "MPEG-4",
        "video/mpeg" => "MPEG",
        other => other.strip_prefix("video/x-").unwrap_or(other),
    };
    match s.get::<&str>("profile") {
        Ok(profile) => format!("{name} {profile}"),
        Err(_) => name.into(),
    }
}
