//! The fonts installed here, by family name.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::OnceLock;

use fontdb::{Database, Family, Query, Source, Stretch, Style, Weight};

/// A font file's face.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontFace {
    pub path: PathBuf,
    /// Which face of a collection.
    pub index: u32,
}

/// Looking through the font folders takes a moment; it is done once.
fn database() -> &'static Database {
    static FONTS: OnceLock<Database> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut db = Database::new();
        db.load_system_fonts();
        tracing::debug!(faces = db.len(), "fonts found");
        db
    })
}

/// The families installed here, sorted.
pub fn font_families() -> Vec<String> {
    let names: BTreeSet<String> = database()
        .faces()
        .filter_map(|f| f.families.first().map(|(name, _)| name.clone()))
        // Hidden ones, and Windows' upright-text twins.
        .filter(|name| !name.is_empty() && !name.starts_with(['.', '@']))
        .collect();
    let mut names: Vec<String> = names.into_iter().collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// The face of `family` nearest to what is asked for; `None` if the family
/// is not installed.
pub fn font_face(family: &str, bold: bool, italic: bool) -> Option<FontFace> {
    let db = database();
    let id = db.query(&Query {
        families: &[Family::Name(family)],
        weight: if bold { Weight::BOLD } else { Weight::NORMAL },
        stretch: Stretch::Normal,
        style: if italic { Style::Italic } else { Style::Normal },
    })?;
    let face = db.face(id)?;
    match &face.source {
        Source::File(path) | Source::SharedFile(path, _) => Some(FontFace {
            path: path.clone(),
            index: face.index,
        }),
        Source::Binary(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listed_families_have_files() {
        let families = font_families();
        if families.is_empty() {
            eprintln!("no fonts here; skipping");
            return;
        }
        assert!(families.iter().all(|f| !f.is_empty()));
        let first = font_face(&families[0], false, false).unwrap();
        assert!(first.path.is_file());
        assert_eq!(font_face("No Such Font At All", false, false), None);
    }
}
