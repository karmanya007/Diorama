//! Conversion to H.264 (pain point P4) with whatever encoder the system's
//! GStreamer has: the GPU's first, then the CPU's. Only the picture is
//! converted; the original's soundtrack keeps playing.

use std::path::Path;

use gstreamer as gst;
use gstreamer::prelude::*;

use super::{GstDecoder, bus_error, error, gst_err, init};
use crate::{Error, MediaSource, Result, VideoDecoder};

/// H.264 encoders, best first (`nvh264enc` is NVENC's old element, which
/// current drivers refuse), and what their `bitrate` property counts in
/// (bits per second).
const ENCODERS: [(&str, u32); 7] = [
    ("vah264enc", 1000),
    ("vah264lpenc", 1000),
    ("nvcudah264enc", 1000),
    ("nvh264enc", 1000),
    ("vaapih264enc", 1000),
    ("x264enc", 1000),
    ("openh264enc", 1),
];

/// Settings to change from the encoder's defaults where it has them: steady
/// bitrate, and no minutes of search for a copy nobody archives.
const SETTINGS: [(&str, &str, &str); 3] = [
    ("openh264enc", "rate-control", "bitrate"),
    ("openh264enc", "complexity", "medium"),
    ("x264enc", "speed-preset", "veryfast"),
];

/// How often progress is reported and cancelling noticed.
const POLL: gst::ClockTime = gst::ClockTime::from_mseconds(200);

/// Converts `src` to an H.264 MP4 at `size`. `progress` gets the fraction
/// done and returns `false` to stop.
pub(crate) fn convert(
    src: &Path,
    dst: &Path,
    size: (u32, u32),
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<()> {
    init()?;
    let info = GstDecoder::open(&MediaSource::File(src.to_path_buf()))?.info();
    let fps = info.frame_rate.unwrap_or(30.0);
    if size.0 >= info.width && !gpu_decodes_h264() {
        return Err(Error::Os {
            call: "convert",
            message: "GStreamer has no H.264 decoder for the graphics card here (VA-API or                       NVDEC), so a copy would cost as much to play"
                .into(),
        });
    }
    let mut failure = Error::Os {
        call: "convert",
        message: "no H.264 encoder is installed: GStreamer's VA-API or NVENC plugins, \
                  openh264 (plugins-bad) or x264 (plugins-ugly)"
            .into(),
    };
    // Starting over with another encoder must not take progress back.
    let mut furthest = 0.0f32;
    let mut progress = |p: f32| {
        furthest = furthest.max(p);
        progress(furthest)
    };
    for (encoder, unit) in ENCODERS {
        if gst::ElementFactory::find(encoder).is_none() {
            continue;
        }
        // About 6 Mbit/s for 1080p30.
        let bits = (f64::from(size.0) * f64::from(size.1) * fps * 0.1).clamp(2e6, 40e6) as u32;
        match run(src, dst, size, fps, (encoder, bits / unit), &mut progress) {
            Ok(()) => {
                tracing::info!(encoder, "converted");
                return Ok(());
            }
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            // The plugin is there but not the hardware, or the size is not
            // one it takes.
            Err(e) => {
                tracing::info!(encoder, "cannot convert with this encoder: {e}");
                failure = e;
            }
        }
    }
    Err(failure)
}

/// Whether a hardware decoder for H.264 is installed.
fn gpu_decodes_h264() -> bool {
    let h264 = gst::Caps::builder("video/x-h264").build();
    gst::ElementFactory::factories_with_type(
        gst::ElementFactoryType::DECODER
            | gst::ElementFactoryType::MEDIA_VIDEO
            | gst::ElementFactoryType::HARDWARE,
        gst::Rank::MARGINAL,
    )
    .iter()
    .any(|f| f.can_sink_any_caps(&h264))
}

fn make(name: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(name)
        .build()
        .map_err(gst_err("gst_element_factory_make"))
}

/// Sets a property if this version of the element has it and knows `value`.
fn set(element: &gst::Element, property: &str, value: &str) {
    let Some(spec) = element.find_property(property) else {
        return;
    };
    match gst::glib::Value::deserialize(value, spec.value_type()) {
        Ok(value) => element.set_property_from_value(property, &value),
        Err(_) => tracing::debug!(property, value, "not known to this encoder"),
    }
}

/// Converts with one encoder (its name and bitrate in its own unit).
fn run(
    src: &Path,
    dst: &Path,
    size: (u32, u32),
    fps: f64,
    (encoder, bitrate): (&str, u32),
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<()> {
    let pipeline = gst::Pipeline::new();
    let source = make("filesrc")?;
    source.set_property("location", src.to_string_lossy().as_ref());
    let decodebin = make("decodebin")?;
    let convert = make("videoconvert")?;
    let scale = make("videoscale")?;
    // Pixels become square without bars: players here ignore the ratio too.
    scale.set_property("add-borders", false);
    let rate = make("videorate")?;
    let caps = make("capsfilter")?;
    caps.set_property(
        "caps",
        gst::Caps::builder("video/x-raw")
            .field("width", size.0 as i32)
            .field("height", size.1 as i32)
            .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
            .field(
                "framerate",
                gst::Fraction::approximate_f64(fps).unwrap_or(gst::Fraction::new(30, 1)),
            )
            .field("interlace-mode", "progressive")
            .build(),
    );
    let encode = make(encoder)?;
    set(&encode, "bitrate", &bitrate.to_string());
    for (_, property, value) in SETTINGS.iter().filter(|(e, ..)| *e == encoder) {
        set(&encode, property, value);
    }
    // Every decoder takes 8-bit 4:2:0; some encoders would keep more.
    let format = make("capsfilter")?;
    format.set_property(
        "caps",
        gst::Caps::builder("video/x-raw")
            .field("format", gst::List::new(["NV12", "I420"]))
            .build(),
    );
    let convert2 = make("videoconvert")?;
    // Some encoders settle for baseline when it is allowed too.
    let mut profiles = gst::Caps::new_empty();
    for profile in ["high", "constrained-high", "main"] {
        profiles.merge(
            gst::Caps::builder("video/x-h264")
                .field("profile", profile)
                .build(),
        );
    }
    let profile = make("capsfilter")?;
    profile.set_property("caps", profiles);
    let parse = make("h264parse")?;
    let mux = make("mp4mux")?;
    let sink = make("filesink")?;
    sink.set_property("location", dst.to_string_lossy().as_ref());

    pipeline
        .add_many([&source, &decodebin])
        .map_err(gst_err("gst_bin_add"))?;
    source.link(&decodebin).map_err(gst_err("link source"))?;
    let chain = [
        &convert, &scale, &rate, &caps, &convert2, &format, &encode, &profile, &parse, &mux, &sink,
    ];
    pipeline.add_many(chain).map_err(gst_err("gst_bin_add"))?;
    gst::Element::link_many(chain).map_err(gst_err("link encoder"))?;
    let input = convert
        .static_pad("sink")
        .ok_or(Error::BadFrame("converter without a sink pad".into()))?;
    decodebin.connect_pad_added(move |_, pad| {
        let video = pad
            .current_caps()
            .or_else(|| Some(pad.query_caps(None)))
            .and_then(|c| c.structure(0).map(|s| s.name().starts_with("video/")))
            .unwrap_or(false);
        if video
            && !input.is_linked()
            && let Err(e) = pad.link(&input)
        {
            tracing::warn!("linking the decoded stream: {e}");
        }
    });
    let result = drive(&pipeline, progress);
    let _ = pipeline.set_state(gst::State::Null);
    result
}

/// Runs `pipeline` to its end.
fn drive(pipeline: &gst::Pipeline, progress: &mut dyn FnMut(f32) -> bool) -> Result<()> {
    let bus = pipeline
        .bus()
        .ok_or(Error::BadFrame("pipeline without a bus".into()))?;
    pipeline.set_state(gst::State::Playing).map_err(|_| {
        bus_error(pipeline).unwrap_or(Error::BadFrame("the encoder did not start".into()))
    })?;
    loop {
        // Messages of other types are dropped, so both ends are asked for.
        let ended = bus.timed_pop_filtered(POLL, &[gst::MessageType::Eos, gst::MessageType::Error]);
        if let Some(message) = ended {
            if let gst::MessageView::Error(e) = message.view() {
                return Err(error(e));
            }
            progress(1.0);
            return Ok(());
        }
        let done = match (
            pipeline.query_position::<gst::ClockTime>(),
            pipeline.query_duration::<gst::ClockTime>(),
        ) {
            (Some(at), Some(all)) if !all.is_zero() => {
                (at.nseconds() as f64 / all.nseconds() as f64).min(1.0) as f32
            }
            _ => 0.0,
        };
        if !progress(done) {
            return Err(Error::Cancelled);
        }
    }
}
