//! Media Foundation video decoder (Windows).
//!
//! Uses the Source Reader with hardware transforms and advanced video
//! processing enabled, asking for NV12 output. No audio stream is opened, so
//! playback never depends on an audio device (pain point P10: WE's video
//! pipeline failed with MF_E_NO_AUDIO_PLAYBACK_DEVICE when no device existed).

pub(crate) mod audio;
pub(crate) mod capture;

use std::time::Duration;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
    D3D11CreateDevice, ID3D11Device, ID3D11Multithread,
};
use windows::Win32::Graphics::Dxgi::IDXGIAdapter;
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer2, IMFAttributes, IMFDXGIDeviceManager, IMFMediaSource, IMFSourceReader,
    MF_BYTESTREAM_CONTENT_TYPE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_OBJECT_INVALID, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
    MF_RESOLUTION_CONTENT_DOES_NOT_HAVE_TO_MATCH_EXTENSION_OR_MIME_TYPE, MF_RESOLUTION_MEDIASOURCE,
    MF_RESOLUTION_READ, MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READER_D3D_MANAGER,
    MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_VERSION, MF2DBuffer_LockFlags_Read, MFCreateAttributes,
    MFCreateDXGIDeviceManager, MFCreateMFByteStreamOnStream, MFCreateMediaType,
    MFCreateSourceReaderFromMediaSource, MFCreateSourceReaderFromURL, MFCreateSourceResolver,
    MFMediaType_Video, MFSTARTUP_FULL, MFShutdown, MFStartup, MFVideoFormat_NV12,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::core::{HSTRING, Interface};

use crate::{Error, MediaSource, Nv12Frame, Result, VideoDecoder, VideoInfo};

const VIDEO_STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

fn os(call: &'static str) -> impl FnOnce(windows::core::Error) -> Error {
    move |e| Error::Os {
        call,
        message: e.message(),
    }
}

/// Keeps COM and Media Foundation initialised on the decoding thread.
struct MfRuntime {
    com: bool,
}

impl MfRuntime {
    fn start() -> Result<Self> {
        // SAFETY: per-thread init, balanced in Drop. S_FALSE (already initialised) is success.
        let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        // SAFETY: MFStartup is reference counted; balanced by MFShutdown in Drop.
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.map_err(os("MFStartup"))?;
        Ok(Self { com })
    }
}

impl Drop for MfRuntime {
    fn drop(&mut self) {
        // SAFETY: balances `start`.
        unsafe {
            let _ = MFShutdown();
            if self.com {
                CoUninitialize();
            }
        }
    }
}

/// D3D11 device handed to Media Foundation so decoders can use DXVA (GPU video
/// engine). Without it the Source Reader decodes in software (measured ~1 CPU
/// core for 4K30).
struct HwDecode {
    manager: IMFDXGIDeviceManager,
    _device: ID3D11Device,
}

impl HwDecode {
    fn create() -> Result<Self> {
        // SAFETY: standard D3D11/MF setup; out-params are valid locals.
        unsafe {
            let mut device = None;
            D3D11CreateDevice(
                None::<&IDXGIAdapter>,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
            .map_err(os("D3D11CreateDevice"))?;
            let device: ID3D11Device = device
                .ok_or_else(|| Error::BadFrame("D3D11CreateDevice returned no device".into()))?;
            // The decoder and the reader touch the device from different threads.
            if let Ok(mt) = device.cast::<ID3D11Multithread>() {
                let _ = mt.SetMultithreadProtected(true);
            }
            let mut token = 0u32;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)
                .map_err(os("MFCreateDXGIDeviceManager"))?;
            let manager =
                manager.ok_or_else(|| Error::BadFrame("no DXGI device manager".into()))?;
            manager
                .ResetDevice(&device, token)
                .map_err(os("IMFDXGIDeviceManager::ResetDevice"))?;
            Ok(Self {
                manager,
                _device: device,
            })
        }
    }
}

pub(crate) struct MfDecoder {
    // Field order: the reader must be released before the D3D manager and Media Foundation.
    reader: IMFSourceReader,
    hw: Option<HwDecode>,
    source: MediaSource,
    info: VideoInfo,
    stride: u32,
    _runtime: MfRuntime,
}

// SAFETY: the reader is created with COINIT_MULTITHREADED and only ever used by
// one thread at a time (the decoder is moved into its thread, never shared).
unsafe impl Send for MfDecoder {}

impl MfDecoder {
    pub(crate) fn open(source: &MediaSource) -> Result<Self> {
        let runtime = MfRuntime::start()?;
        let hw = HwDecode::create()
            .inspect_err(|e| {
                tracing::warn!("hardware video decode unavailable, using software: {e}")
            })
            .ok();
        let (reader, info, stride) = match open_reader(source, hw.as_ref()) {
            Ok(r) => r,
            // Some codecs/drivers refuse the D3D path; software still works.
            Err(e) if hw.is_some() => {
                tracing::warn!("hardware decode setup failed ({e}); retrying in software");
                open_reader(source, None)?
            }
            Err(e) => return Err(e),
        };
        tracing::info!(
            width = info.width,
            height = info.height,
            fps = ?info.frame_rate,
            hardware = hw.is_some(),
            "video opened"
        );
        Ok(Self {
            reader,
            hw,
            source: source.clone(),
            info,
            stride,
            _runtime: runtime,
        })
    }
}

fn open_reader(
    source: &MediaSource,
    hw: Option<&HwDecode>,
) -> Result<(IMFSourceReader, VideoInfo, u32)> {
    // SAFETY: COM calls on interfaces we own; GUID pointers reference constants.
    unsafe {
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 3).map_err(os("MFCreateAttributes"))?;
        let attrs = attrs.ok_or_else(|| Error::BadFrame("no attributes".into()))?;
        if let Some(hw) = hw {
            attrs
                .SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, &hw.manager)
                .map_err(os("SetUnknown(D3D manager)"))?;
        }
        // Hardware (DXVA) decoders where available; the video processor converts
        // whatever the decoder outputs into NV12.
        attrs
            .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
            .map_err(os("SetUINT32(hw transforms)"))?;
        attrs
            .SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
            .map_err(os("SetUINT32(video processing)"))?;

        let reader = source_reader(source, &attrs)?;
        // Video only: never touch audio streams or devices.
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .map_err(os("SetStreamSelection(all)"))?;
        reader.SetStreamSelection(VIDEO_STREAM, true).map_err(|_| {
            Error::NoVideo(match source {
                MediaSource::File(p) => p.clone(),
                MediaSource::Memory { name, .. } => name.into(),
            })
        })?;

        let wanted = MFCreateMediaType().map_err(os("MFCreateMediaType"))?;
        wanted
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(os("SetGUID(major)"))?;
        wanted
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
            .map_err(os("SetGUID(subtype)"))?;
        reader
            .SetCurrentMediaType(VIDEO_STREAM, None, &wanted)
            .map_err(os("SetCurrentMediaType(NV12)"))?;

        let current = reader
            .GetCurrentMediaType(VIDEO_STREAM)
            .map_err(os("GetCurrentMediaType"))?;
        let size = current
            .GetUINT64(&MF_MT_FRAME_SIZE)
            .map_err(os("MF_MT_FRAME_SIZE"))?;
        let (width, height) = ((size >> 32) as u32, size as u32);
        let frame_rate = current.GetUINT64(&MF_MT_FRAME_RATE).ok().and_then(|r| {
            let (num, den) = ((r >> 32) as u32, r as u32);
            (den != 0).then(|| f64::from(num) / f64::from(den))
        });
        // Negative stride means bottom-up rows; NV12 from MF is top-down in practice.
        let stride = current
            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
            .map(|s| (s as i32).unsigned_abs())
            .unwrap_or(width);

        Ok((
            reader,
            VideoInfo {
                width,
                height,
                frame_rate,
            },
            stride.max(width),
        ))
    }
}

/// A source reader over `source` (a file, or bytes in memory).
pub(crate) fn source_reader(
    source: &MediaSource,
    attrs: &IMFAttributes,
) -> Result<IMFSourceReader> {
    // SAFETY: COM calls on interfaces we own; the memory stream copies the bytes.
    unsafe {
        match source {
            MediaSource::File(path) => {
                MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), attrs)
                    .map_err(os("MFCreateSourceReaderFromURL"))
            }
            MediaSource::Memory { data, .. } => {
                let stream = SHCreateMemStream(Some(data))
                    .ok_or_else(|| Error::BadFrame("SHCreateMemStream failed".into()))?;
                let byte_stream = MFCreateMFByteStreamOnStream(&stream)
                    .map_err(os("MFCreateMFByteStreamOnStream"))?;
                if let (Some(mime), Ok(a)) = (source.mime(), byte_stream.cast::<IMFAttributes>()) {
                    let _ = a.SetString(&MF_BYTESTREAM_CONTENT_TYPE, &HSTRING::from(mime));
                }
                // A bare stream does not identify MP3 or WAV: resolve with the
                // file name as URL so the extension picks the parser.
                let resolver = MFCreateSourceResolver().map_err(os("MFCreateSourceResolver"))?;
                let mut kind = MF_OBJECT_INVALID;
                let mut object = None;
                resolver
                    .CreateObjectFromByteStream(
                        &byte_stream,
                        &HSTRING::from(source.name()),
                        (MF_RESOLUTION_MEDIASOURCE.0
                            | MF_RESOLUTION_CONTENT_DOES_NOT_HAVE_TO_MATCH_EXTENSION_OR_MIME_TYPE.0
                            | MF_RESOLUTION_READ.0) as u32,
                        None::<&IPropertyStore>,
                        &mut kind,
                        &mut object,
                    )
                    .map_err(os("CreateObjectFromByteStream"))?;
                let media_source = object
                    .ok_or_else(|| Error::BadFrame("no media source".into()))?
                    .cast::<IMFMediaSource>()
                    .map_err(os("IMFMediaSource"))?;
                MFCreateSourceReaderFromMediaSource(&media_source, attrs)
                    .map_err(os("MFCreateSourceReaderFromMediaSource"))
            }
        }
    }
}

impl VideoDecoder for MfDecoder {
    fn info(&self) -> VideoInfo {
        self.info
    }

    fn next_frame(&mut self) -> Result<Option<Nv12Frame>> {
        loop {
            let mut flags = 0u32;
            let mut timestamp = 0i64;
            let mut sample = None;
            // SAFETY: out-params are valid locals.
            unsafe {
                self.reader
                    .ReadSample(
                        VIDEO_STREAM,
                        0,
                        None,
                        Some(&mut flags),
                        Some(&mut timestamp),
                        Some(&mut sample),
                    )
                    .map_err(os("ReadSample"))?;
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                return Ok(None);
            }
            // Stream ticks / format changes can arrive without a sample.
            let Some(sample) = sample else { continue };

            let pts = Duration::from_nanos((timestamp.max(0) as u64) * 100);
            // SAFETY: buffers locked only for the duration of the copies below.
            unsafe {
                let buffer = sample
                    .ConvertToContiguousBuffer()
                    .map_err(os("ConvertToContiguousBuffer"))?;
                // 2D buffers know their real row pitch: decoders pad rows
                // (e.g. 1028 → 1088 bytes), which the media type may not say.
                if let Ok(b2) = buffer.cast::<IMF2DBuffer2>() {
                    let (mut scan0, mut pitch) = (std::ptr::null_mut(), 0i32);
                    let (mut start, mut len) = (std::ptr::null_mut(), 0u32);
                    if b2
                        .Lock2DSize(
                            MF2DBuffer_LockFlags_Read,
                            &mut scan0,
                            &mut pitch,
                            &mut start,
                            &mut len,
                        )
                        .is_ok()
                    {
                        // Top-down rows only (NV12 from MF is top-down in practice).
                        let ok = pitch > 0 && scan0 == start;
                        let bytes =
                            ok.then(|| std::slice::from_raw_parts(start, len as usize).to_vec());
                        b2.Unlock2D().map_err(os("IMF2DBuffer::Unlock2D"))?;
                        if let Some(bytes) = bytes {
                            self.stride = pitch.unsigned_abs();
                            return nv12_from_buffer(bytes, self.info, self.stride, pts).map(Some);
                        }
                    }
                }
                let mut ptr = std::ptr::null_mut();
                let mut len = 0u32;
                buffer
                    .Lock(&mut ptr, None, Some(&mut len))
                    .map_err(os("IMFMediaBuffer::Lock"))?;
                let bytes = std::slice::from_raw_parts(ptr, len as usize).to_vec();
                buffer.Unlock().map_err(os("IMFMediaBuffer::Unlock"))?;
                return nv12_from_buffer(bytes, self.info, self.stride, pts).map(Some);
            }
        }
    }

    fn restart(&mut self) -> Result<()> {
        // Reopening is simpler and as fast as seeking for looping wallpapers.
        let (reader, info, stride) = open_reader(&self.source, self.hw.as_ref())?;
        self.reader = reader;
        self.info = info;
        self.stride = stride;
        Ok(())
    }
}

/// Splits an NV12 buffer. Decoders pad the luma plane height (e.g. 1080 → 1088),
/// so the chroma offset comes from the buffer length, not the visible height.
fn nv12_from_buffer(
    data: Vec<u8>,
    info: VideoInfo,
    stride: u32,
    pts: Duration,
) -> Result<Nv12Frame> {
    let stride_us = stride as usize;
    let visible = stride_us * info.height as usize;
    if stride_us == 0 || data.len() < visible + visible / 2 {
        return Err(Error::BadFrame(format!(
            "{} bytes for {}x{} stride {stride}",
            data.len(),
            info.width,
            info.height
        )));
    }
    let padded_rows = data.len() * 2 / (3 * stride_us);
    Ok(Nv12Frame {
        width: info.width,
        height: info.height,
        stride,
        chroma_offset: stride_us * padded_rows.max(info.height as usize),
        data,
        pts,
        loop_index: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chroma_offset_accounts_for_padded_height() {
        let info = VideoInfo {
            width: 1920,
            height: 1080,
            frame_rate: None,
        };
        // 1088 padded rows of luma + 544 rows of chroma.
        let data = vec![0u8; 1920 * 1088 * 3 / 2];
        let f = nv12_from_buffer(data, info, 1920, Duration::ZERO).unwrap();
        assert_eq!(f.chroma_offset, 1920 * 1088);
        assert_eq!(f.chroma().len(), 1920 * 544);
    }

    #[test]
    fn rejects_short_buffers() {
        let info = VideoInfo {
            width: 64,
            height: 64,
            frame_rate: None,
        };
        assert!(nv12_from_buffer(vec![0; 100], info, 64, Duration::ZERO).is_err());
    }
}
