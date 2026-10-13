//! Resource caps applied when reading untrusted content (Workshop items).
//!
//! Every parser takes a `&Limits` so a malformed or hostile file fails with an
//! error instead of exhausting memory.

#[derive(Debug, Clone)]
pub struct Limits {
    /// Maximum number of entries in one `.pkg` archive.
    pub max_pkg_entries: u32,
    /// Maximum length in bytes of a path or other length-prefixed string.
    pub max_string_len: u32,
    /// Maximum size of a single decompressed buffer (texture mip, etc.).
    pub max_decompressed_bytes: u64,
    /// Maximum texture width or height in pixels.
    pub max_texture_dim: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_pkg_entries: 65_536,
            max_string_len: 4_096,
            max_decompressed_bytes: 512 * 1024 * 1024,
            max_texture_dim: 16_384,
        }
    }
}
