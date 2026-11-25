//! Documents → scene IR, without a GPU.

#![allow(clippy::unwrap_used, reason = "test")]

use std::path::Path;
use std::sync::Arc;

use diorama_core::Limits;
use diorama_project::{
    Cache, Error, Folder, Host, Memory, NoHost, Values, compile, compile_document,
};
use diorama_scene::{Blend, Scene, ShaderLanguage, TextureBinding, VideoData};
use serde_json::{Value, json};

fn png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::write_buffer_with_format(
        &mut out,
        &rgba.repeat((width * height) as usize),
        width,
        height,
        image::ExtendedColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .unwrap();
    out.into_inner()
}

fn files(document: &Value) -> Memory {
    let mut files = Memory::default();
    files.0.insert(
        "wallpaper.json".into(),
        serde_json::to_vec(document).unwrap(),
    );
    files
        .0
        .insert("assets/red.png".into(), png(4, 2, [255, 0, 0, 255]));
    files
}

fn scene(document: &Value) -> Scene {
    compile(
        &files(document),
        &Values::new(),
        &NoHost,
        &Limits::default(),
    )
    .unwrap()
}

#[test]
fn layers_are_placed_in_scene_coordinates() {
    let s = scene(&json!({
        "version": 1, "size": [200, 100], "background": "#102030",
        "parallax": {"amount": 0.1},
        "layers": [
            {"type": "image", "source": "assets/red.png", "name": "Red", "position": [10, 20],
             "scale": [2, 3], "rotation": 90, "opacity": 0.5, "depth": 0.25},
            {"type": "color", "id": "shade", "color": "#00ff00", "blend": "add", "visible": false},
            {"type": "image", "source": "assets/red.png", "size": 50, "blend": "multiply"},
        ],
    }));
    assert!(s.skipped.is_empty(), "{:?}", s.skipped);
    assert_eq!((s.width, s.height), (200, 100));
    assert_eq!(
        s.clear_color,
        [16.0 / 255.0, 32.0 / 255.0, 48.0 / 255.0, 1.0]
    );
    assert_eq!(s.parallax.unwrap().amount, 0.1);
    let [red, shade, multiplied] = &s.layers[..] else {
        panic!("{} layers", s.layers.len());
    };
    assert_eq!((red.name.as_str(), red.size), ("Red", [4, 2]));
    assert_eq!(red.color, [1.0, 1.0, 1.0, 0.5]);
    assert_eq!(red.parallax_depth, [0.25, 0.25]);
    // Right and down of the middle; y counts up in scenes.
    let [m0, m1, m2, m3, x, y] = red.transform;
    assert_eq!([x, y], [110.0, 30.0]);
    // A quarter turn clockwise: the layer's right points down.
    let near = |a: f32, b: f32| (a - b).abs() < 1e-5;
    assert!(near(m0, 0.0) && near(m1, -2.0) && near(m2, 3.0) && near(m3, 0.0));
    assert_eq!(
        (shade.name.as_str(), shade.size, shade.visible),
        ("shade", [200, 100], false)
    );
    assert_eq!(shade.blend, Blend::Additive);
    assert_eq!(multiplied.size, [50, 50]);
    assert_eq!(
        multiplied.albedo.textures.get(&1),
        Some(&TextureBinding::FrameBuffer)
    );
    // Shared textures and programs; nothing that needs DXC.
    assert_eq!((s.textures.len(), s.programs.len()), (2, 2));
    assert!(
        s.programs
            .iter()
            .all(|p| p.language == ShaderLanguage::Wgsl)
    );
}

#[test]
fn properties_set_values() {
    let document = json!({
        "version": 1, "size": [10, 10], "background": {"property": "sky"},
        "layers": [{"type": "color", "color": {"property": "accent"}, "opacity": {"property": "strength"},
                    "visible": {"property": "shown"}}],
    });
    let mut files = files(&document);
    let project = json!({"type": "diorama", "general": {"properties": {
        "sky": {"type": "color", "value": "0 0 1"},
        "accent": {"type": "color", "value": "1 0 0"},
        "strength": {"type": "slider", "value": 1},
        "shown": {"type": "bool", "value": true},
    }}});
    files
        .0
        .insert("project.json".into(), serde_json::to_vec(&project).unwrap());
    let choices: Values = [
        ("strength".to_owned(), json!(0.25)),
        ("shown".to_owned(), json!(false)),
    ]
    .into();
    let s = compile(&files, &choices, &NoHost, &Limits::default()).unwrap();
    assert!(s.skipped.is_empty(), "{:?}", s.skipped);
    assert_eq!(s.clear_color, [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(s.layers[0].color, [1.0, 0.0, 0.0, 0.25]);
    assert!(!s.layers[0].visible);
}

#[test]
fn unusable_layers_are_left_out_and_named() {
    let s = scene(&json!({
        "version": 1, "size": [10, 10],
        "layers": [
            {"type": "image", "name": "Lost", "source": "assets/none.png"},
            {"type": "image", "name": "Out", "source": "../secret.png"},
            {"type": "hologram", "name": "Odd"},
            {"name": "Nothing"},
            {"type": "text", "name": "Words", "text": "no font here"},
            {"type": "video", "name": "Film", "source": "assets/red.png"},
            7,
            {"type": "color", "id": "a", "effects": [{"type": "sparkle"}, {"type": "mask"}, 3]},
            {"type": "adjustment", "name": "Empty"},
            {"type": "color", "id": "a", "opacity": "much"},
        ],
    }));
    assert_eq!(s.layers.len(), 2);
    for name in [
        "Lost",
        "Out",
        "Odd",
        "Nothing",
        "Words",
        "Film",
        "layer 7",
        "effects",
        "another layer",
        "opacity",
    ] {
        assert!(
            s.skipped.iter().any(|p| p.contains(name)),
            "{name}: {:?}",
            s.skipped
        );
    }
}

#[test]
fn documents_must_have_version_and_size() {
    let error = |document: Value| {
        compile(
            &files(&document),
            &Values::new(),
            &NoHost,
            &Limits::default(),
        )
        .err()
    };
    assert!(matches!(
        error(json!({"version": 2, "size": [1, 1]})),
        Some(Error::Newer(2))
    ));
    for bad in [
        json!([]),
        json!({"size": [1, 1]}),
        json!({"version": 1}),
        json!({"version": 1, "size": "big"}),
    ] {
        assert!(matches!(error(bad), Some(Error::Document(_))));
    }
    // Sizes stay within what a GPU takes.
    let s = scene(&json!({"version": 1, "size": [0, 1e9], "layers": "none"}));
    assert_eq!((s.width, s.height), (1, Limits::default().max_texture_dim));
    assert_eq!(s.skipped.len(), 1);
}

#[test]
fn images_count_together() {
    let document = json!({
        "version": 1, "size": [10, 10],
        "layers": [
            {"type": "image", "source": "assets/red.png"},
            {"type": "image", "source": "assets/red.png", "smooth": false},
        ],
    });
    let limits = Limits {
        max_decompressed_bytes: 40,
        ..Limits::default()
    };
    let s = compile(&files(&document), &Values::new(), &NoHost, &limits).unwrap();
    assert_eq!(s.layers.len(), 1);
    assert_eq!(s.skipped.len(), 1);
}

struct Videos;

impl Host for Videos {
    fn default_font(&self) -> Option<Arc<[u8]>> {
        None
    }

    fn system_font(&self, family: &str, _bold: bool, _italic: bool) -> Option<(Arc<[u8]>, u32)> {
        (family == "Known").then(|| Some((self.default_font()?, 0)))?
    }

    fn video_size(&self, file: &Path) -> Option<[u32; 2]> {
        file.ends_with("assets/film.mp4").then_some([64, 32])
    }
}

#[test]
fn videos_stay_on_disk() {
    let dir = std::env::temp_dir().join(format!("diorama-project-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("project.json"), r#"{"type": "Diorama"}"#).unwrap();
    std::fs::write(
        dir.join("wallpaper.json"),
        r#"{"version": 1, "size": [128, 64], "layers": [{"type": "video", "source": "assets/film.mp4"}]}"#,
    )
    .unwrap();
    std::fs::write(dir.join("assets/film.mp4"), "film").unwrap();
    assert!(diorama_project::is_project(&dir));
    let s = diorama_project::load(&dir, &Values::new(), &Videos, &Limits::default()).unwrap();
    assert!(s.skipped.is_empty(), "{:?}", s.skipped);
    assert_eq!(s.layers[0].size, [64, 32]);
    assert!(matches!(&s.textures[0].video, Some(VideoData::File(f)) if f.is_file()));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!diorama_project::is_project(&dir));
}

/// A short stand-in for the fuzz target: broken documents and images are
/// errors or left-out layers, never panics.
#[test]
fn broken_input_does_not_panic() {
    let document = json!({
        "version": 1, "size": [200, 100], "background": "#102030",
        "parallax": {"amount": {"property": "p"}},
        "layers": [
            {"type": "image", "source": "assets/red.png", "position": [10, 20], "scale": [2, 3],
             "rotation": 1e30, "opacity": 0.5, "blend": "screen", "depth": -2, "size": [1, 2]},
            {"type": "text", "text": "words", "font": "assets/red.png", "max-lines": 2},
            {"type": "color", "color": "#12345678"},
        ],
    });
    let original = files(&document);
    let limits = Limits {
        max_texture_dim: 256,
        ..Limits::default()
    };
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut random = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    let bytes = b"0123456789-+.eE\"{}[]:,#\x00\xff";
    for _ in 0..4000 {
        let mut files = original.clone();
        let name = ["wallpaper.json", "assets/red.png"][random(2)];
        let data = files.0.get_mut(name).unwrap();
        for _ in 0..=random(4) {
            let (at, byte) = (random(data.len()), bytes[random(bytes.len())]);
            match random(3) {
                0 => data[at] = byte,
                1 => data.insert(at, byte),
                _ => {
                    data.remove(at);
                }
            }
        }
        let _ = compile(&files, &Values::new(), &NoHost, &limits);
    }
}

#[test]
fn unchanged_files_are_not_read_again() {
    let dir = std::env::temp_dir().join(format!("diorama-cache-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    for name in ["a.png", "b.png"] {
        std::fs::write(dir.join("assets").join(name), png(2, 2, [1, 2, 3, 255])).unwrap();
    }
    let folder = Folder::new(&dir);
    let mut cache = Cache::default();
    let mut compiled = |layers: Value| {
        let document = json!({"version": 1, "size": [10, 10], "layers": layers});
        compile_document(
            &document,
            &folder,
            &Values::new(),
            &NoHost,
            &Limits::default(),
            &mut cache,
        )
        .unwrap()
    };
    let image = |name: &str| json!({"type": "image", "source": format!("assets/{name}")});
    let first = compiled(json!([image("a.png"), {"type": "text"}, image("b.png")]));
    // The text has no font here: layers keep their numbers in the document.
    let documents: Vec<usize> = first.layers.iter().map(|l| l.document).collect();
    assert_eq!(documents, [0, 2]);
    assert_eq!(first.layers[0].size, Some([2.0, 2.0]));

    std::fs::write(dir.join("assets/b.png"), png(5, 2, [9, 9, 9, 255])).unwrap();
    let second = compiled(json!([image("b.png"), image("a.png")]));
    let (a, b) = (&first.scene.textures, &second.scene.textures);
    assert!(Arc::ptr_eq(&a[0], &b[1]), "a.png is the same");
    assert!(!Arc::ptr_eq(&a[1], &b[0]), "b.png was changed");
    assert_eq!(second.layers[0].size, Some([5.0, 2.0]));
    let _ = std::fs::remove_dir_all(&dir);
}
