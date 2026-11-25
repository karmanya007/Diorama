//! Diorama's own wallpapers (ADR-009, `docs/formats/diorama.md`): a folder
//! with `project.json` (title, tags, preview, user properties; `"type":
//! "diorama"`), `wallpaper.json` (the document) and assets. The document is
//! compiled to the scene IR; nothing here knows Wallpaper Engine's formats.
//!
//! Wallpapers are shared files, so everything read is untrusted: sizes are
//! limited and assets stay inside the folder. A layer that cannot be used is
//! left out and named in [`Scene::skipped`].

mod assets;
mod compile;
mod effects;
mod fields;
mod picture;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use diorama_core::Limits;
use diorama_scene::Scene;
use serde_json::Value;

pub use assets::{Assets, Folder, Memory};
pub use compile::VERSION;
pub use effects::{Effect, Kind, Setting, effects};

/// The document's file name.
pub const DOCUMENT: &str = "wallpaper.json";
/// Largest `project.json` or document read.
const MAX_JSON: u64 = 16 << 20;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {message}")]
    Asset { path: String, message: String },
    #[error("{DOCUMENT}: {0}")]
    Document(String),
    #[error("made with a newer Diorama (format {0}; this one reads up to {VERSION})")]
    Newer(u64),
}

/// User property values by name.
pub type Values = BTreeMap<String, Value>;

/// What compiling needs from the system.
pub trait Host {
    /// The font for text that names none.
    fn default_font(&self) -> Option<Arc<[u8]>>;
    /// A video's size in pixels.
    fn video_size(&self, file: &Path) -> Option<[u32; 2]>;
}

/// Neither fonts nor videos (tests, tools).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoHost;

impl Host for NoHost {
    fn default_font(&self) -> Option<Arc<[u8]>> {
        None
    }

    fn video_size(&self, _file: &Path) -> Option<[u32; 2]> {
        None
    }
}

/// Whether `dir` holds a wallpaper of Diorama's own.
pub fn is_project(dir: &Path) -> bool {
    project(&Folder::new(dir)).is_some_and(|p| {
        p.get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| t.eq_ignore_ascii_case("diorama"))
    })
}

/// Compiles the wallpaper in `dir`, with `choices` over the property defaults
/// in its `project.json`.
pub fn load(dir: &Path, choices: &Values, host: &dyn Host, limits: &Limits) -> Result<Scene> {
    compile(&Folder::new(dir), choices, host, limits)
}

/// Like [`load`], from any assets.
pub fn compile(
    assets: &dyn Assets,
    choices: &Values,
    host: &dyn Host,
    limits: &Limits,
) -> Result<Scene> {
    let text = assets.read(DOCUMENT, MAX_JSON)?;
    let document = parse(&text).map_err(|e| Error::Document(e.to_string()))?;
    let mut values = defaults(project(assets).as_ref());
    values.extend(choices.iter().map(|(k, v)| (k.clone(), v.clone())));
    compile::compile(&document, assets, &values, host, limits)
}

fn parse(text: &[u8]) -> serde_json::Result<Value> {
    serde_json::from_slice(text.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(text))
}

fn project(assets: &dyn Assets) -> Option<Value> {
    parse(&assets.read("project.json", MAX_JSON).ok()?).ok()
}

/// Property defaults (`general.properties` of `project.json`).
fn defaults(project: Option<&Value>) -> Values {
    project
        .and_then(|p| p.get("general")?.get("properties")?.as_object())
        .into_iter()
        .flatten()
        .filter_map(|(name, p)| Some((name.clone(), p.get("value")?.clone())))
        .collect()
}
