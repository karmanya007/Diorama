//! Bounds-checked little-endian cursor over a byte slice.

use crate::error::{Error, Result};

#[derive(Debug)]
pub(crate) struct ByteReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> ByteReader<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let truncated = Error::Truncated {
            offset: self.pos,
            needed: n,
        };
        let end = self.pos.checked_add(n).ok_or(truncated)?;
        let slice = self.buf.get(self.pos..end).ok_or(Error::Truncated {
            offset: self.pos,
            needed: n,
        })?;
        self.pos = end;
        Ok(slice)
    }

    pub(crate) fn u32(&mut self) -> Result<u32> {
        let offset = self.pos;
        let raw: [u8; 4] = self
            .bytes(4)?
            .try_into()
            .map_err(|_| Error::Truncated { offset, needed: 4 })?;
        Ok(u32::from_le_bytes(raw))
    }

    /// `u32` byte length followed by that many UTF-8 bytes.
    pub(crate) fn string_u32(&mut self, max_len: u32, what: &'static str) -> Result<&'a str> {
        let len = self.u32()?;
        if len > max_len {
            return Err(Error::LimitExceeded {
                what,
                value: u64::from(len),
                limit: u64::from(max_len),
            });
        }
        let raw = self.bytes(len as usize)?;
        std::str::from_utf8(raw).map_err(|_| Error::InvalidUtf8 { what })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_le_u32_and_string() {
        let data = [2, 0, 0, 0, b'h', b'i', 7, 0, 0, 0];
        let mut r = ByteReader::new(&data);
        assert_eq!(r.string_u32(16, "test").unwrap(), "hi");
        assert_eq!(r.u32().unwrap(), 7);
        assert_eq!(r.pos(), data.len());
    }

    #[test]
    fn truncation_is_an_error_not_a_panic() {
        let mut r = ByteReader::new(&[1, 2]);
        assert!(matches!(
            r.u32(),
            Err(Error::Truncated {
                offset: 0,
                needed: 4
            })
        ));
    }

    #[test]
    fn string_length_is_capped() {
        let data = [0xff, 0xff, 0xff, 0x7f];
        let mut r = ByteReader::new(&data);
        assert!(matches!(
            r.string_u32(16, "test"),
            Err(Error::LimitExceeded { .. })
        ));
    }
}
