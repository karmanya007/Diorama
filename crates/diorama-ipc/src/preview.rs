//! What the editor's preview process (`dioramad --preview`) and the UI say to
//! each other (ADR-009): commands as JSON lines on its standard input, and
//! on its standard output messages of a kind byte, a little-endian length
//! and that many bytes.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// Widest and tallest preview.
pub const MAX_SIDE: u32 = 4096;
/// Longest text message.
const MAX_TEXT: u32 = 1024 * 1024;

const FRAME: u8 = 1;
const EVENT: u8 = 2;
/// Width and height before a frame's pixels.
const FRAME_HEADER: u32 = 8;

/// What the UI asks of the preview; absent fields stay as they are.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Command {
    /// Size of the frames to send.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<[u32; 2]>,
    /// Most frames per second (0: none are sent).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    /// Cursor over the frame, 0..1, y down.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pointer: Option<[f32; 2]>,
}

/// What the preview reports beside frames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    /// The wallpaper is up; its own size.
    Ready {
        width: u32,
        height: u32,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// `width` × `height` RGBA pixels, rows from the top.
    Frame {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Event(Event),
}

pub fn write_frame(out: &mut impl Write, width: u32, height: u32, rgba: &[u8]) -> io::Result<()> {
    if rgba.len() as u64 != pixels(width, height)? {
        return Err(bad("frame size"));
    }
    let length = u32::try_from(rgba.len()).map_err(|_| bad("frame size"))? + FRAME_HEADER;
    let mut head = [0; 13];
    head[0] = FRAME;
    head[1..5].copy_from_slice(&length.to_le_bytes());
    head[5..9].copy_from_slice(&width.to_le_bytes());
    head[9..13].copy_from_slice(&height.to_le_bytes());
    out.write_all(&head)?;
    out.write_all(rgba)?;
    out.flush()
}

pub fn write_event(out: &mut impl Write, event: &Event) -> io::Result<()> {
    let text = serde_json::to_vec(event).map_err(io::Error::other)?;
    let length = u32::try_from(text.len())
        .ok()
        .filter(|n| *n <= MAX_TEXT)
        .ok_or_else(|| bad("event size"))?;
    out.write_all(&[EVENT])?;
    out.write_all(&length.to_le_bytes())?;
    out.write_all(&text)?;
    out.flush()
}

/// The next message; `None` when the other side has ended.
pub fn read(input: &mut impl Read) -> io::Result<Option<Message>> {
    let mut kind = [0];
    if input.read(&mut kind)? == 0 {
        return Ok(None);
    }
    let length = read_u32(input)?;
    match kind[0] {
        FRAME => {
            let (width, height) = (read_u32(input)?, read_u32(input)?);
            let size = pixels(width, height)?;
            if u64::from(length) != size + u64::from(FRAME_HEADER) {
                return Err(bad("frame size"));
            }
            let mut rgba = vec![0; usize::try_from(size).map_err(|_| bad("frame size"))?];
            input.read_exact(&mut rgba)?;
            Ok(Some(Message::Frame {
                width,
                height,
                rgba,
            }))
        }
        EVENT if length <= MAX_TEXT => {
            let mut text = vec![0; length as usize];
            input.read_exact(&mut text)?;
            let event = serde_json::from_slice(&text).map_err(io::Error::other)?;
            Ok(Some(Message::Event(event)))
        }
        _ => Err(bad("message")),
    }
}

fn read_u32(input: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0; 4];
    input.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

/// Bytes of a frame, if its size is allowed.
fn pixels(width: u32, height: u32) -> io::Result<u64> {
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(bad("frame size"));
    }
    Ok(u64::from(width) * u64::from(height) * 4)
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("preview: bad {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip() {
        let rgba: Vec<u8> = (0..24).collect();
        let ready = Event::Ready {
            width: 1920,
            height: 1080,
        };
        let mut pipe = Vec::new();
        write_event(&mut pipe, &ready).unwrap();
        write_frame(&mut pipe, 3, 2, &rgba).unwrap();
        let mut input = pipe.as_slice();
        assert_eq!(read(&mut input).unwrap(), Some(Message::Event(ready)));
        assert_eq!(
            read(&mut input).unwrap(),
            Some(Message::Frame {
                width: 3,
                height: 2,
                rgba
            })
        );
        assert_eq!(read(&mut input).unwrap(), None);
    }

    #[test]
    fn bad_messages_are_errors() {
        let mut out = Vec::new();
        assert!(write_frame(&mut out, 2, 2, &[0; 15]).is_err());
        assert!(write_frame(&mut out, 0, 2, &[]).is_err());
        assert!(write_frame(&mut out, MAX_SIDE + 1, 1, &[0; 16388]).is_err());
        assert!(out.is_empty());

        let mut frame = Vec::new();
        write_frame(&mut frame, 1, 1, &[1, 2, 3, 4]).unwrap();
        // Cut short, another length, a huge size, an unknown kind.
        assert!(read(&mut &frame[..frame.len() - 1]).is_err());
        let mut longer = frame.clone();
        longer[1] += 4;
        assert!(read(&mut longer.as_slice()).is_err());
        let mut huge = frame.clone();
        huge[5..9].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read(&mut huge.as_slice()).is_err());
        assert!(read(&mut [9, 0, 0, 0, 0].as_slice()).is_err());
        assert!(read(&mut [EVENT, 0, 0, 0, 255].as_slice()).is_err());
    }

    #[test]
    fn commands_name_only_what_changes() {
        let command: Command = serde_json::from_str(r#"{"size":[960,540]}"#).unwrap();
        assert_eq!(command.size, Some([960, 540]));
        assert_eq!(command.fps, None);
        assert_eq!(
            serde_json::to_string(&command).unwrap(),
            r#"{"size":[960,540]}"#
        );
    }
}
