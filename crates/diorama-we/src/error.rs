pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unexpected end of data at offset {offset} (needed {needed} more bytes)")]
    Truncated { offset: usize, needed: usize },

    #[error("bad magic: expected {expected}, found {found:?}")]
    BadMagic {
        expected: &'static str,
        found: String,
    },

    #[error("{what} exceeds limit: {value} > {limit}")]
    LimitExceeded {
        what: &'static str,
        value: u64,
        limit: u64,
    },

    #[error("invalid UTF-8 in {what}")]
    InvalidUtf8 { what: &'static str },

    #[error("unsafe entry path {0:?}")]
    UnsafePath(String),

    #[error("duplicate entry path {0:?}")]
    DuplicatePath(String),

    #[error("unsupported {what}: {value}")]
    Unsupported { what: &'static str, value: String },

    #[error("LZ4 decompression failed: {0}")]
    Lz4(String),

    #[error("{what}: expected {expected} bytes, got {actual}")]
    SizeMismatch {
        what: &'static str,
        expected: u64,
        actual: u64,
    },

    #[error("shader: {0}")]
    Shader(String),

    #[error("image decode failed: {0}")]
    Image(String),

    #[error("entry {path:?} out of bounds (offset {offset}, size {size}, data length {data_len})")]
    EntryOutOfBounds {
        path: String,
        offset: u64,
        size: u64,
        data_len: u64,
    },
}
