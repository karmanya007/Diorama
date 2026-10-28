//! Wallpaper Engine importer (L2).
//!
//! All input handled here is untrusted Workshop content: parsers never panic,
//! check every length against [`diorama_core::Limits`], and reject unsafe paths.

pub mod config;
pub mod error;
pub mod language;
pub mod mdl;
pub mod pkg;
mod reader;
pub mod scene;
pub mod shader;
pub mod tex;
pub mod tex_decode;

pub use error::{Error, Result};
pub use pkg::{Pkg, PkgEntry};
pub use tex::{Tex, TexFlags, TexFormat};
pub use tex_decode::{Rgba8Image, mip_to_rgba8};
