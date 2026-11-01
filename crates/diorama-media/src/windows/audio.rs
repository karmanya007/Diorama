//! Media Foundation track reader for sound playback: decodes a file's audio
//! stream to interleaved float.

use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSourceReader, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READERF_ENDOFSTREAM, MFAudioFormat_Float,
    MFCreateAttributes, MFCreateMediaType, MFMediaType_Audio,
};

use super::{MfRuntime, os, source_reader};
use crate::{Error, MediaSource, Result};

const AUDIO_STREAM: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;

/// Keeps Media Foundation running on the decoding thread.
pub(crate) fn decode_thread() -> Result<impl Sized> {
    MfRuntime::start()
}

pub(crate) struct TrackReader {
    reader: IMFSourceReader,
    rate: u32,
    channels: usize,
}

impl TrackReader {
    pub(crate) fn open(source: &MediaSource) -> Result<Self> {
        let (reader, rate, channels) = open_audio_reader(source)?;
        Ok(Self {
            reader,
            rate,
            channels,
        })
    }

    /// Sample rate and channel count of what [`Self::read`] appends.
    pub(crate) fn format(&self) -> (u32, usize) {
        (self.rate, self.channels)
    }

    /// Appends the next decoded samples (interleaved) to `out`; `false` at
    /// the end of the track.
    pub(crate) fn read(&mut self, out: &mut Vec<f32>) -> Result<bool> {
        let mut flags = 0u32;
        let mut sample = None;
        // SAFETY: out-params are valid locals.
        unsafe {
            self.reader
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
            return Ok(false);
        }
        let Some(sample) = sample else {
            return Ok(true);
        };
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
            out.extend_from_slice(std::slice::from_raw_parts(
                ptr.cast::<f32>(),
                len as usize / 4,
            ));
            buffer
                .Unlock()
                .map_err(os("IMFMediaBuffer::Unlock(audio)"))?;
        }
        Ok(true)
    }
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
