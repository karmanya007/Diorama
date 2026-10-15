//! `wpdump` — inspect and unpack Wallpaper Engine content.
//!
//! Output directories may contain copyrighted Workshop content; keep them out
//! of the repository (`/extracted/` is git-ignored).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use lumen_core::Limits;
use lumen_we::tex_decode::freeimage_format_name;
use lumen_we::{Pkg, Tex, TexFlags, mip_to_rgba8};

mod shader_check;
mod survey;

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List the entries of a .pkg archive.
    List { pkg: PathBuf },
    /// Extract every entry of a .pkg archive into a directory.
    Unpack { pkg: PathBuf, out_dir: PathBuf },
    /// Print one entry of a .pkg archive to stdout (e.g. scene.json).
    Cat { pkg: PathBuf, entry: String },
    /// Describe a .tex file (or a .tex entry inside a .pkg with --entry).
    TexInfo {
        input: PathBuf,
        #[arg(long)]
        entry: Option<String>,
    },
    /// Convert the first mip of a .tex to PNG.
    Tex2png {
        input: PathBuf,
        out: PathBuf,
        #[arg(long)]
        entry: Option<String>,
        /// Crop to the header's image size instead of the padded texture size.
        #[arg(long)]
        crop: bool,
    },
    /// Aggregate feature usage across a Workshop folder (Markdown report, counts only).
    Survey {
        #[arg(long, default_value = DEFAULT_CORPUS)]
        corpus: PathBuf,
        #[arg(long, default_value = DEFAULT_INSTALL)]
        install: PathBuf,
        /// Write the report here instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Translate every shader pair to GLSL 450 and validate with naga (ADR-003 spike).
    ShaderCheck {
        #[arg(long, default_value = DEFAULT_CORPUS)]
        corpus: PathBuf,
        #[arg(long, default_value = DEFAULT_INSTALL)]
        install: PathBuf,
        /// Write translated sources of failing pairs here.
        #[arg(long)]
        dump: Option<PathBuf>,
        /// Set every declared combo to 1 instead of its default.
        #[arg(long)]
        all_combos_on: bool,
        #[arg(long, value_enum, default_value = "glsl")]
        backend: shader_check::Backend,
        /// Path to dxc.exe (required for --backend hlsl).
        #[arg(long)]
        dxc: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Lay out text like a WE text object and print the glyph quads
    /// (pixels relative to the object origin, y up) and the text box.
    Text {
        /// Font file (.ttf/.otf), or a .pkg with --entry.
        font: PathBuf,
        text: String,
        #[arg(long)]
        entry: Option<String>,
        /// WE point size (rendered at 300 dpi).
        #[arg(long, default_value_t = 32.0)]
        pointsize: f32,
        #[arg(long, default_value = "center")]
        halign: String,
        #[arg(long, default_value = "center")]
        valign: String,
        /// Write the glyph atlas here.
        #[arg(long)]
        atlas: Option<PathBuf>,
    },
}

const DEFAULT_CORPUS: &str = r"C:\Program Files (x86)\Steam\steamapps\workshop\content\431960";
const DEFAULT_INSTALL: &str = r"C:\Program Files (x86)\Steam\steamapps\common\wallpaper_engine";

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        // Output piped into `head` / `Select -First N` closed early: not an error.
        Err(e)
            if e.downcast_ref::<io::Error>()
                .is_some_and(|io| io.kind() == io::ErrorKind::BrokenPipe) =>
        {
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let limits = Limits::default();
    let mut out = io::stdout().lock();

    match cli.command {
        Command::List { pkg } => {
            let bytes = read(&pkg)?;
            let archive = Pkg::parse(&bytes, &limits)?;
            writeln!(
                out,
                "{} — {} entries",
                archive.version(),
                archive.entries().len()
            )?;
            for entry in archive.entries() {
                writeln!(out, "{:>12}  {}", entry.size, entry.path)?;
            }
        }
        Command::Unpack { pkg, out_dir } => {
            let bytes = read(&pkg)?;
            let archive = Pkg::parse(&bytes, &limits)?;
            unpack(&archive, &out_dir)?;
            writeln!(
                out,
                "extracted {} entries to {}",
                archive.entries().len(),
                out_dir.display()
            )?;
        }
        Command::Cat { pkg, entry } => {
            let bytes = read(&pkg)?;
            let archive = Pkg::parse(&bytes, &limits)?;
            let Some(data) = archive.get(&entry) else {
                bail!("no entry {entry:?} in {}", pkg.display());
            };
            out.write_all(data)?;
        }
        Command::TexInfo { input, entry } => {
            let bytes = read_input(&input, entry.as_deref(), &limits)?;
            let tex = Tex::parse(&bytes, &limits)?;
            describe(&mut out, &tex)?;
        }
        Command::Survey {
            corpus,
            install,
            out: report_path,
        } => {
            let report = survey::run(&corpus, &install)?;
            match report_path {
                Some(path) => fs::write(&path, report)
                    .with_context(|| format!("writing {}", path.display()))?,
                None => out.write_all(report.as_bytes())?,
            }
        }
        Command::ShaderCheck {
            corpus,
            install,
            dump,
            all_combos_on,
            backend,
            dxc,
            out: report_path,
        } => {
            let report = shader_check::run(&shader_check::Options {
                corpus: &corpus,
                install: &install,
                dump: dump.as_deref(),
                all_combos_on,
                backend,
                dxc: dxc.as_deref(),
            })?;
            match report_path {
                Some(path) => fs::write(&path, report)
                    .with_context(|| format!("writing {}", path.display()))?,
                None => out.write_all(report.as_bytes())?,
            }
        }
        Command::Text {
            font,
            text,
            entry,
            pointsize,
            halign,
            valign,
            atlas,
        } => {
            let bytes = read_input(&font, entry.as_deref(), &limits)?;
            let font = lumen_text::Font::new(bytes.into(), 0).context("loading font")?;
            let style = lumen_text::Style {
                pixel_size: pointsize * 300.0 / 72.0,
                halign: match halign.as_str() {
                    "left" => lumen_text::HAlign::Left,
                    "right" => lumen_text::HAlign::Right,
                    _ => lumen_text::HAlign::Center,
                },
                valign: match valign.as_str() {
                    "top" => lumen_text::VAlign::Top,
                    "bottom" => lumen_text::VAlign::Bottom,
                    _ => lumen_text::VAlign::Center,
                },
                max_width: None,
                max_rows: None,
                ellipsis: false,
            };
            let mut engine = lumen_text::TextEngine::new();
            let layout = engine.layout(&font, &text, &style);
            writeln!(out, "metrics {:?}", font.metrics(style.pixel_size))?;
            for q in &layout.quads {
                writeln!(out, "quad {:?} atlas {:?}", q.rect, q.atlas)?;
            }
            writeln!(
                out,
                "bounds {:?} ({} x {}), rows {}",
                layout.bounds,
                layout.width(),
                layout.height(),
                layout.rows
            )?;
            if let Some(path) = atlas {
                let a = engine.atlas();
                image::GrayImage::from_raw(a.size(), a.size(), a.pixels().to_vec())
                    .context("atlas size mismatch")?
                    .save(&path)
                    .with_context(|| format!("writing {}", path.display()))?;
            }
        }
        Command::Tex2png {
            input,
            out: png_path,
            entry,
            crop,
        } => {
            let bytes = read_input(&input, entry.as_deref(), &limits)?;
            let tex = Tex::parse(&bytes, &limits)?;
            let mip = tex.first_mip().context("texture has no mips")?;
            let mut rgba = mip_to_rgba8(&tex, mip)?;
            if tex.header.format == lumen_we::TexFormat::R8 {
                // Shaders see (r, 0, 0, 1); a grey preview is easier to read.
                for p in rgba.pixels.as_chunks_mut::<4>().0 {
                    p[1] = p[0];
                    p[2] = p[0];
                }
            }
            let mut img = image::RgbaImage::from_raw(rgba.width, rgba.height, rgba.pixels)
                .context("pixel buffer size mismatch")?;
            if crop {
                let w = tex.header.image_width.min(img.width());
                let h = tex.header.image_height.min(img.height());
                img = image::imageops::crop_imm(&img, 0, 0, w, h).to_image();
            }
            img.save(&png_path)
                .with_context(|| format!("writing {}", png_path.display()))?;
            writeln!(
                out,
                "wrote {}×{} {}",
                img.width(),
                img.height(),
                png_path.display()
            )?;
        }
    }
    Ok(())
}

fn read(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).with_context(|| format!("reading {}", path.display()))
}

/// A loose file, or one entry of a `.pkg` when `entry` is given.
fn read_input(path: &Path, entry: Option<&str>, limits: &Limits) -> Result<Vec<u8>> {
    let bytes = read(path)?;
    let Some(entry) = entry else {
        return Ok(bytes);
    };
    let archive = Pkg::parse(&bytes, limits)?;
    archive
        .get(entry)
        .map(<[u8]>::to_vec)
        .with_context(|| format!("no entry {entry:?} in {}", path.display()))
}

fn describe(out: &mut impl Write, tex: &Tex) -> Result<()> {
    let h = &tex.header;
    let flag_names = [
        (TexFlags::NO_INTERPOLATION, "no-interpolation"),
        (TexFlags::CLAMP_UVS, "clamp-uvs"),
        (TexFlags::SPRITE_SHEET, "sprite-sheet"),
        (TexFlags::VIDEO, "video"),
        (TexFlags::EXTRA_DEPTH, "extra-depth"),
    ];
    let known: Vec<_> = flag_names
        .iter()
        .filter(|(bit, _)| h.flags.contains(*bit))
        .map(|(_, name)| *name)
        .collect();
    writeln!(out, "container   TEXB{:04}", tex.container_version)?;
    writeln!(out, "format      {:?}", h.format)?;
    writeln!(out, "flags       {:#x} [{}]", h.flags.0, known.join(", "))?;
    writeln!(
        out,
        "texture     {}×{} (image {}×{})",
        h.texture_width, h.texture_height, h.image_width, h.image_height
    )?;
    if let Some(extra) = h.extra {
        writeln!(out, "extra       {extra}")?;
    }
    match tex.image_format {
        Some(id) => writeln!(
            out,
            "encoding    {} (FreeImage {id})",
            freeimage_format_name(id)
        )?,
        None if tex.is_video => writeln!(out, "encoding    mp4 video")?,
        None => writeln!(out, "encoding    raw")?,
    }
    for (i, image) in tex.images.iter().enumerate() {
        let mips: Vec<_> = image
            .mips
            .iter()
            .map(|m| format!("{}×{}", m.width, m.height))
            .collect();
        writeln!(out, "image {i}     {} mips: {}", mips.len(), mips.join(" "))?;
    }
    if let Some(sheet) = &tex.sprite_sheet {
        let total: f32 = sheet.frames.iter().map(|f| f.frametime).sum();
        writeln!(
            out,
            "sprites     TEXS{:04}, {} frames, {total:.3}s loop",
            sheet.version,
            sheet.frames.len()
        )?;
    }
    Ok(())
}

fn unpack(archive: &Pkg<'_>, out_dir: &Path) -> Result<()> {
    fs::create_dir_all(out_dir)?;
    let root = out_dir.canonicalize()?;
    for entry in archive.entries() {
        // Entry paths are validated by Pkg::parse; re-check the joined path anyway.
        let target = root.join(&entry.path);
        if !target.starts_with(&root) {
            bail!(
                "refusing to write outside {}: {}",
                root.display(),
                entry.path
            );
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = archive
            .data(entry)
            .with_context(|| format!("entry {} out of bounds", entry.path))?;
        fs::write(&target, data).with_context(|| format!("writing {}", target.display()))?;
    }
    Ok(())
}
