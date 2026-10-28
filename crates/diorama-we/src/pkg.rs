//! Reader for Wallpaper Engine `.pkg` archives (`scene.pkg`).
//!
//! Layout, all integers little-endian `u32` (see `docs/formats/pkg.md`):
//!
//! ```text
//! string  version        u32 length + bytes, e.g. "PKGV0001"
//! u32     entry_count
//! entry_count × {
//!     string  path       '/'-separated, relative
//!     u32     offset     relative to the start of the data section
//!     u32     size
//! }
//! data section           starts immediately after the table
//! ```

use std::collections::HashSet;

use diorama_core::Limits;

use crate::error::{Error, Result};
use crate::reader::ByteReader;

const MAGIC_PREFIX: &str = "PKGV";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkgEntry {
    pub path: String,
    /// Absolute offset into the archive bytes.
    pub offset: usize,
    pub size: usize,
}

/// Parsed archive borrowing the underlying bytes (read from disk or memory-mapped).
#[derive(Debug)]
pub struct Pkg<'a> {
    version: &'a str,
    entries: Vec<PkgEntry>,
    bytes: &'a [u8],
}

impl<'a> Pkg<'a> {
    pub fn parse(bytes: &'a [u8], limits: &Limits) -> Result<Self> {
        let mut r = ByteReader::new(bytes);

        let version = r.string_u32(limits.max_string_len, "pkg version")?;
        if version.len() != 8 || !version.starts_with(MAGIC_PREFIX) {
            return Err(Error::BadMagic {
                expected: "PKGVxxxx",
                found: version.to_owned(),
            });
        }

        let count = r.u32()?;
        if count > limits.max_pkg_entries {
            return Err(Error::LimitExceeded {
                what: "pkg entry count",
                value: u64::from(count),
                limit: u64::from(limits.max_pkg_entries),
            });
        }

        let mut raw = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let path = r.string_u32(limits.max_string_len, "pkg entry path")?;
            let offset = r.u32()? as usize;
            let size = r.u32()? as usize;
            raw.push((path, offset, size));
        }

        let data_start = r.pos();
        let data_len = bytes.len() - data_start;
        let mut seen = HashSet::with_capacity(raw.len());
        let mut entries = Vec::with_capacity(raw.len());

        for (path, offset, size) in raw {
            validate_entry_path(path)?;
            if !seen.insert(path) {
                return Err(Error::DuplicatePath(path.to_owned()));
            }
            let in_bounds = offset.checked_add(size).is_some_and(|end| end <= data_len);
            if !in_bounds {
                return Err(Error::EntryOutOfBounds {
                    path: path.to_owned(),
                    offset: offset as u64,
                    size: size as u64,
                    data_len: data_len as u64,
                });
            }
            entries.push(PkgEntry {
                path: path.to_owned(),
                offset: data_start + offset,
                size,
            });
        }

        Ok(Self {
            version,
            entries,
            bytes,
        })
    }

    /// Format version string, e.g. `"PKGV0001"`.
    pub fn version(&self) -> &'a str {
        self.version
    }

    pub fn entries(&self) -> &[PkgEntry] {
        &self.entries
    }

    pub fn get(&self, path: &str) -> Option<&'a [u8]> {
        self.entries
            .iter()
            .find(|e| e.path == path)
            .and_then(|e| self.data(e))
    }

    /// Bytes of an entry belonging to this archive. Bounds were checked in [`Pkg::parse`].
    pub fn data(&self, entry: &PkgEntry) -> Option<&'a [u8]> {
        self.bytes
            .get(entry.offset..entry.offset.checked_add(entry.size)?)
    }
}

/// Rejects anything that could escape an extraction directory: absolute paths,
/// drive letters, `.`/`..` components, empty components, backslashes and NULs.
fn validate_entry_path(path: &str) -> Result<()> {
    let unsafe_path = || Error::UnsafePath(path.to_owned());
    if path.is_empty() || path.contains(['\\', ':', '\0']) {
        return Err(unsafe_path());
    }
    for component in path.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(unsafe_path());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(version: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
        fn put_str(out: &mut Vec<u8>, s: &str) {
            out.extend_from_slice(&(s.len() as u32).to_le_bytes());
            out.extend_from_slice(s.as_bytes());
        }
        let mut out = Vec::new();
        put_str(&mut out, version);
        out.extend_from_slice(&(files.len() as u32).to_le_bytes());
        let mut offset = 0u32;
        for (path, data) in files {
            put_str(&mut out, path);
            out.extend_from_slice(&offset.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            offset += data.len() as u32;
        }
        for (_, data) in files {
            out.extend_from_slice(data);
        }
        out
    }

    #[test]
    fn parses_entries_and_reads_data() {
        let bytes = build(
            "PKGV0001",
            &[("scene.json", b"{}"), ("materials/a.tex", b"TEXV")],
        );
        let pkg = Pkg::parse(&bytes, &Limits::default()).unwrap();
        assert_eq!(pkg.version(), "PKGV0001");
        assert_eq!(pkg.entries().len(), 2);
        assert_eq!(pkg.get("scene.json"), Some(&b"{}"[..]));
        assert_eq!(pkg.get("materials/a.tex"), Some(&b"TEXV"[..]));
        assert_eq!(pkg.get("missing"), None);
    }

    #[test]
    fn rejects_bad_magic() {
        let bytes = build("ZIPV0001", &[]);
        assert!(matches!(
            Pkg::parse(&bytes, &Limits::default()),
            Err(Error::BadMagic { .. })
        ));
    }

    #[test]
    fn rejects_path_traversal() {
        for bad in [
            "../evil",
            "a/../../evil",
            "/abs",
            "C:/x",
            "a\\b",
            "a//b",
            "./a",
            "",
        ] {
            let bytes = build("PKGV0001", &[(bad, b"x")]);
            assert!(
                matches!(
                    Pkg::parse(&bytes, &Limits::default()),
                    Err(Error::UnsafePath(_))
                ),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn rejects_duplicate_paths() {
        let bytes = build("PKGV0001", &[("a", b"1"), ("a", b"2")]);
        assert!(matches!(
            Pkg::parse(&bytes, &Limits::default()),
            Err(Error::DuplicatePath(_))
        ));
    }

    #[test]
    fn rejects_out_of_bounds_entry() {
        let mut bytes = build("PKGV0001", &[("a", b"1234")]);
        bytes.truncate(bytes.len() - 1);
        assert!(matches!(
            Pkg::parse(&bytes, &Limits::default()),
            Err(Error::EntryOutOfBounds { .. })
        ));
    }

    #[test]
    fn caps_entry_count() {
        let bytes = build("PKGV0001", &[("a", b"1"), ("b", b"2")]);
        let limits = Limits {
            max_pkg_entries: 1,
            ..Limits::default()
        };
        assert!(matches!(
            Pkg::parse(&bytes, &limits),
            Err(Error::LimitExceeded { .. })
        ));
    }

    #[test]
    fn truncated_table_is_an_error() {
        let bytes = build("PKGV0001", &[("scene.json", b"{}")]);
        assert!(matches!(
            Pkg::parse(&bytes[..14], &Limits::default()),
            Err(Error::Truncated { .. })
        ));
    }
}
