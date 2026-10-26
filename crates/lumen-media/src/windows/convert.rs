//! Hardware-decode probe and conversion to H.264 (pain point P4): a video
//! whose codec this GPU cannot decode costs a CPU core or more to play; an
//! H.264 copy at the size the monitors need decodes on any GPU.

use std::path::Path;

use windows::Win32::Graphics::Direct3D11::{
    D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0, D3D11_DECODER_PROFILE_H264_VLD_NOFGT,
    D3D11_DECODER_PROFILE_HEVC_VLD_MAIN, D3D11_DECODER_PROFILE_HEVC_VLD_MAIN10,
    D3D11_DECODER_PROFILE_MPEG2_VLD, D3D11_DECODER_PROFILE_VC1_D2010,
    D3D11_DECODER_PROFILE_VP9_VLD_10BIT_PROFILE2, D3D11_DECODER_PROFILE_VP9_VLD_PROFILE0,
    D3D11_VIDEO_DECODER_DESC, ID3D11VideoDevice,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_NV12, DXGI_FORMAT_P010};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFMediaType, IMFSinkWriter, IMFSourceReader, MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
    MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
    MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_MT_VIDEO_PROFILE, MF_PD_DURATION,
    MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_ANY_STREAM, MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READER_MEDIASOURCE,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR, MF_SOURCE_READERF_STREAMTICK,
    MFAudioFormat_AAC, MFAudioFormat_PCM, MFCreateAttributes, MFCreateMediaType,
    MFCreateSinkWriterFromURL, MFCreateSourceReaderFromURL, MFMediaType_Audio, MFMediaType_Video,
    MFVideoFormat_AV1, MFVideoFormat_H264, MFVideoFormat_HEVC, MFVideoFormat_MPEG2,
    MFVideoFormat_NV12, MFVideoFormat_VP90, MFVideoFormat_WVC1, MFVideoInterlace_Progressive,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToUInt64;
use windows::core::{GUID, HSTRING, Interface};

use super::{MfRuntime, VIDEO_STREAM, os, video_device};
use crate::{Error, Result, VideoProbe};

const AUDIO_STREAM: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
/// H.264 profiles with more than 8 bits or 4:2:0 (High 10, 4:2:2, 4:4:4),
/// which GPUs do not decode.
const H264_BEYOND_HIGH: [u32; 3] = [110, 122, 244];
/// `eAVEncH265VProfile_Main_420_10`.
const HEVC_MAIN10: u32 = 2;

pub(crate) fn probe(path: &Path) -> Result<VideoProbe> {
    let _runtime = MfRuntime::start()?;
    // SAFETY: COM calls on interfaces we own; descriptors are plain data.
    unsafe {
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None)
            .map_err(os("MFCreateSourceReaderFromURL"))?;
        let native = reader
            .GetNativeMediaType(VIDEO_STREAM, 0)
            .map_err(|_| Error::NoVideo(path.to_path_buf()))?;
        let subtype = native
            .GetGUID(&MF_MT_SUBTYPE)
            .map_err(os("MF_MT_SUBTYPE"))?;
        let size = native.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
        let (width, height) = ((size >> 32) as u32, size as u32);
        let (codec, profiles) =
            decoder_profiles(subtype, native.GetUINT32(&MF_MT_VIDEO_PROFILE).ok());
        let video = video_device()
            .ok()
            .and_then(|d| d.cast::<ID3D11VideoDevice>().ok());
        let hardware = video.is_some_and(|video| {
            profiles.iter().any(|&(guid, format)| {
                let desc = D3D11_VIDEO_DECODER_DESC {
                    Guid: guid,
                    SampleWidth: width,
                    SampleHeight: height,
                    OutputFormat: format,
                };
                video.GetVideoDecoderConfigCount(&desc).is_ok_and(|n| n > 0)
            })
        });
        Ok(VideoProbe {
            codec,
            width,
            height,
            hardware,
        })
    }
}

/// The codec's name and the DXVA decoder profiles that would decode it.
fn decoder_profiles(subtype: GUID, profile: Option<u32>) -> (String, Vec<(GUID, DXGI_FORMAT)>) {
    let (name, profiles): (&str, Vec<(GUID, DXGI_FORMAT)>) = match subtype {
        s if s == MFVideoFormat_H264 => match profile {
            Some(p) if H264_BEYOND_HIGH.contains(&p) => ("H.264 (10-bit or 4:2:2/4:4:4)", vec![]),
            _ => (
                "H.264",
                vec![(D3D11_DECODER_PROFILE_H264_VLD_NOFGT, DXGI_FORMAT_NV12)],
            ),
        },
        s if s == MFVideoFormat_HEVC => match profile {
            Some(HEVC_MAIN10) => (
                "HEVC 10-bit",
                vec![(D3D11_DECODER_PROFILE_HEVC_VLD_MAIN10, DXGI_FORMAT_P010)],
            ),
            _ => (
                "HEVC",
                vec![(D3D11_DECODER_PROFILE_HEVC_VLD_MAIN, DXGI_FORMAT_NV12)],
            ),
        },
        s if s == MFVideoFormat_VP90 => (
            "VP9",
            vec![
                (D3D11_DECODER_PROFILE_VP9_VLD_PROFILE0, DXGI_FORMAT_NV12),
                (
                    D3D11_DECODER_PROFILE_VP9_VLD_10BIT_PROFILE2,
                    DXGI_FORMAT_P010,
                ),
            ],
        ),
        s if s == MFVideoFormat_AV1 => (
            "AV1",
            vec![
                (D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0, DXGI_FORMAT_NV12),
                (D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0, DXGI_FORMAT_P010),
            ],
        ),
        s if s == MFVideoFormat_WVC1 => (
            "VC-1",
            vec![(D3D11_DECODER_PROFILE_VC1_D2010, DXGI_FORMAT_NV12)],
        ),
        s if s == MFVideoFormat_MPEG2 => (
            "MPEG-2",
            vec![(D3D11_DECODER_PROFILE_MPEG2_VLD, DXGI_FORMAT_NV12)],
        ),
        // Media subtypes are FOURCCs in the GUID's first field.
        s => {
            let fourcc: String = s
                .data1
                .to_le_bytes()
                .iter()
                .map(|&b| if b.is_ascii_graphic() { b as char } else { '?' })
                .collect();
            return (fourcc, vec![]);
        }
    };
    (name.to_owned(), profiles)
}

/// Converts `src` to an H.264 + AAC MP4 at `size` (`dst` must end in
/// `.mp4`). `progress` gets the fraction done and returns `false` to stop.
pub(crate) fn convert(
    src: &Path,
    dst: &Path,
    size: (u32, u32),
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<()> {
    let _runtime = MfRuntime::start()?;
    // SAFETY: COM calls on interfaces we own; samples pass straight from the
    // reader to the writer.
    unsafe {
        let attrs = attributes()?;
        attrs
            .SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
            .map_err(os("SetUINT32(video processing)"))?;
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(src.as_os_str()), &attrs)
            .map_err(os("MFCreateSourceReaderFromURL"))?;
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .map_err(os("SetStreamSelection(all)"))?;
        reader
            .SetStreamSelection(VIDEO_STREAM, true)
            .map_err(|_| Error::NoVideo(src.to_path_buf()))?;

        // Decoded to NV12 at the new size (the video processor scales).
        let native = reader
            .GetNativeMediaType(VIDEO_STREAM, 0)
            .map_err(os("GetNativeMediaType(video)"))?;
        let rate = native
            .GetUINT64(&MF_MT_FRAME_RATE)
            .ok()
            .filter(|r| *r as u32 != 0)
            .unwrap_or(30 << 32 | 1);
        let wanted = media_type(&MFMediaType_Video, &MFVideoFormat_NV12)?;
        set_u64(
            &wanted,
            &MF_MT_FRAME_SIZE,
            u64::from(size.0) << 32 | u64::from(size.1),
        )?;
        set_u64(&wanted, &MF_MT_FRAME_RATE, rate)?;
        reader
            .SetCurrentMediaType(VIDEO_STREAM, None, &wanted)
            .map_err(os("SetCurrentMediaType(NV12)"))?;
        let video_in = reader
            .GetCurrentMediaType(VIDEO_STREAM)
            .map_err(os("GetCurrentMediaType(video)"))?;
        let frame = video_in
            .GetUINT64(&MF_MT_FRAME_SIZE)
            .map_err(os("MF_MT_FRAME_SIZE"))?;
        let (width, height) = ((frame >> 32) as u32, frame as u32);
        let fps = f64::from((rate >> 32) as u32) / f64::from((rate as u32).max(1));

        let video_out = media_type(&MFMediaType_Video, &MFVideoFormat_H264)?;
        set_u64(&video_out, &MF_MT_FRAME_SIZE, frame)?;
        set_u64(&video_out, &MF_MT_FRAME_RATE, rate)?;
        set_u64(&video_out, &MF_MT_PIXEL_ASPECT_RATIO, 1 << 32 | 1)?;
        set_u32(
            &video_out,
            &MF_MT_INTERLACE_MODE,
            MFVideoInterlace_Progressive.0 as u32,
        )?;
        // About 6 Mbit/s for 1080p30.
        let bits = (f64::from(width) * f64::from(height) * fps * 0.1).clamp(2e6, 40e6);
        set_u32(&video_out, &MF_MT_AVG_BITRATE, bits as u32)?;

        let audio = audio_types(&reader);
        if audio.is_none() {
            let _ = reader.SetStreamSelection(AUDIO_STREAM, false);
        }
        let (writer, video, audio) = match open_writer(dst, &video_in, &video_out, audio.as_ref()) {
            Ok(w) => w,
            Err(e) if audio.is_some() => {
                tracing::warn!("converting without the soundtrack: {e}");
                let _ = reader.SetStreamSelection(AUDIO_STREAM, false);
                let _ = std::fs::remove_file(dst);
                open_writer(dst, &video_in, &video_out, None)?
            }
            Err(e) => return Err(e),
        };
        let duration = reader
            .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
            .ok()
            .and_then(|v| PropVariantToUInt64(&v).ok())
            .unwrap_or(0);

        writer.BeginWriting().map_err(os("BeginWriting"))?;
        let video_stream = video_index(&reader);
        let (mut video_done, mut audio_done) = (false, audio.is_none());
        while !(video_done && audio_done) {
            let (mut index, mut flags, mut time) = (0u32, 0u32, 0i64);
            let mut sample = None;
            reader
                .ReadSample(
                    MF_SOURCE_READER_ANY_STREAM.0 as u32,
                    0,
                    Some(&mut index),
                    Some(&mut flags),
                    Some(&mut time),
                    Some(&mut sample),
                )
                .map_err(os("ReadSample"))?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                return Err(Error::BadFrame("the source failed while converting".into()));
            }
            let is_video = video_stream == Some(index);
            let out = if is_video { Some(video) } else { audio };
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                if is_video {
                    video_done = true;
                } else {
                    audio_done = true;
                }
            }
            let Some(out) = out else { continue };
            if flags & MF_SOURCE_READERF_STREAMTICK.0 as u32 != 0 {
                writer
                    .SendStreamTick(out, time)
                    .map_err(os("SendStreamTick"))?;
            }
            let Some(sample) = sample else { continue };
            writer
                .WriteSample(out, &sample)
                .map_err(os("WriteSample"))?;
            let done = (time.max(0) as f64 / duration.max(1) as f64).min(1.0) as f32;
            if is_video && !progress(done) {
                return Err(Error::Cancelled);
            }
        }
        writer.Finalize().map_err(os("Finalize"))?;
        progress(1.0);
        Ok(())
    }
}

fn attributes() -> Result<IMFAttributes> {
    let mut attrs: Option<IMFAttributes> = None;
    // SAFETY: out-param is a valid local.
    unsafe {
        MFCreateAttributes(&mut attrs, 2).map_err(os("MFCreateAttributes"))?;
        let attrs = attrs.ok_or_else(|| Error::BadFrame("no attributes".into()))?;
        attrs
            .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
            .map_err(os("SetUINT32(hw transforms)"))?;
        Ok(attrs)
    }
}

fn media_type(major: &GUID, subtype: &GUID) -> Result<IMFMediaType> {
    // SAFETY: new media type we own.
    unsafe {
        let t = MFCreateMediaType().map_err(os("MFCreateMediaType"))?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, major)
            .map_err(os("SetGUID(major)"))?;
        t.SetGUID(&MF_MT_SUBTYPE, subtype)
            .map_err(os("SetGUID(subtype)"))?;
        Ok(t)
    }
}

fn set_u32(t: &IMFMediaType, key: &GUID, value: u32) -> Result<()> {
    // SAFETY: plain attribute write.
    unsafe { t.SetUINT32(key, value) }.map_err(os("SetUINT32"))
}

fn set_u64(t: &IMFMediaType, key: &GUID, value: u64) -> Result<()> {
    // SAFETY: plain attribute write.
    unsafe { t.SetUINT64(key, value) }.map_err(os("SetUINT64"))
}

/// The soundtrack as 16-bit PCM in, AAC out — if there is one the AAC
/// encoder takes (44.1 or 48 kHz, mono or stereo).
fn audio_types(reader: &IMFSourceReader) -> Option<(IMFMediaType, IMFMediaType)> {
    // SAFETY: COM calls on interfaces we own.
    unsafe {
        reader.SetStreamSelection(AUDIO_STREAM, true).ok()?;
        let pcm = media_type(&MFMediaType_Audio, &MFAudioFormat_PCM).ok()?;
        pcm.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).ok()?;
        reader.SetCurrentMediaType(AUDIO_STREAM, None, &pcm).ok()?;
        let input = reader.GetCurrentMediaType(AUDIO_STREAM).ok()?;
        let rate = input.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).ok()?;
        let channels = input.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).ok()?;
        if !matches!(rate, 44_100 | 48_000) || !matches!(channels, 1 | 2) {
            tracing::warn!(
                rate,
                channels,
                "soundtrack format the AAC encoder does not take"
            );
            return None;
        }
        let aac = media_type(&MFMediaType_Audio, &MFAudioFormat_AAC).ok()?;
        aac.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).ok()?;
        aac.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate).ok()?;
        aac.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels).ok()?;
        // 128 kbit/s.
        aac.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 16_000)
            .ok()?;
        Some((input, aac))
    }
}

/// The writer with its video stream and, if given, its audio stream.
fn open_writer(
    dst: &Path,
    video_in: &IMFMediaType,
    video_out: &IMFMediaType,
    audio: Option<&(IMFMediaType, IMFMediaType)>,
) -> Result<(IMFSinkWriter, u32, Option<u32>)> {
    // SAFETY: COM calls on interfaces we own.
    unsafe {
        let attrs = attributes()?;
        let writer = MFCreateSinkWriterFromURL(&HSTRING::from(dst.as_os_str()), None, &attrs)
            .map_err(os("MFCreateSinkWriterFromURL"))?;
        let video = writer
            .AddStream(video_out)
            .map_err(os("AddStream(H.264)"))?;
        writer
            .SetInputMediaType(video, video_in, None)
            .map_err(os("SetInputMediaType(NV12)"))?;
        let audio = match audio {
            Some((input, output)) => {
                let stream = writer.AddStream(output).map_err(os("AddStream(AAC)"))?;
                writer
                    .SetInputMediaType(stream, input, None)
                    .map_err(os("SetInputMediaType(PCM)"))?;
                Some(stream)
            }
            None => None,
        };
        Ok((writer, video, audio))
    }
}

/// Real index of the selected video stream (`ReadSample` on any stream
/// reports real indices, not the first-video alias).
fn video_index(reader: &IMFSourceReader) -> Option<u32> {
    // SAFETY: queries on the reader; stops at the first invalid index.
    unsafe {
        for i in 0..64 {
            let selected = reader.GetStreamSelection(i).ok()?;
            let major = reader
                .GetCurrentMediaType(i)
                .and_then(|t| t.GetGUID(&MF_MT_MAJOR_TYPE));
            if selected.as_bool() && major.is_ok_and(|m| m == MFMediaType_Video) {
                return Some(i);
            }
        }
    }
    None
}
