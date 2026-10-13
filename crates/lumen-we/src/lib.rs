//! Wallpaper Engine importer (L2).
//!
//! All input handled here is untrusted Workshop content: parsers never panic,
//! check every length against [`lumen_core::Limits`], and reject unsafe paths.

pub mod error;
pub mod pkg;
mod reader;

pub use error::{Error, Result};
pub use pkg::{Pkg, PkgEntry};
