//! `imgdiff <reference> <candidate>`: SSIM/PSNR, optional diff and side-by-side images.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use image::{RgbaImage, imageops};

#[derive(Debug, Parser)]
#[command(about)]
struct Args {
    reference: PathBuf,
    candidate: PathBuf,
    /// Cover-fit the candidate to the reference size first (scene vs presented frame).
    #[arg(long)]
    cover: bool,
    /// Stretch the candidate to the reference size instead (WE "stretch" alignment).
    #[arg(long, conflicts_with = "cover")]
    stretch: bool,
    /// Write |reference − candidate| × 4 here.
    #[arg(long)]
    diff: Option<PathBuf>,
    /// Print mean RGBA of both images (spot empty or black targets).
    #[arg(long)]
    stats: bool,
    /// Write reference | candidate side by side (half size) here.
    #[arg(long)]
    side_by_side: Option<PathBuf>,
    /// Compare only this region `x,y,w,h` (after fitting); the side-by-side
    /// image is then full size.
    #[arg(long, value_parser = parse_rect)]
    crop: Option<[u32; 4]>,
}

fn parse_rect(s: &str) -> Result<[u32; 4], String> {
    let v: Vec<u32> = s
        .split(',')
        .map(|p| p.trim().parse().map_err(|e| format!("{p}: {e}")))
        .collect::<Result<_, _>>()?;
    v.try_into().map_err(|_| "expected x,y,w,h".to_owned())
}

fn open(path: &PathBuf) -> Result<RgbaImage> {
    Ok(image::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .to_rgba8())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let reference = open(&args.reference)?;
    let mut candidate = open(&args.candidate)?;
    if args.stats {
        for (name, img) in [("reference", &reference), ("candidate", &candidate)] {
            let n = f64::from(img.width()) * f64::from(img.height());
            let mut sum = [0f64; 4];
            for p in img.pixels() {
                for (s, c) in sum.iter_mut().zip(p.0) {
                    *s += f64::from(c);
                }
            }
            println!("{name}: mean RGBA {:?}", sum.map(|s| (s / n).round()));
        }
    }
    let (w, h) = reference.dimensions();
    if args.stretch {
        candidate = imageops::resize(&candidate, w, h, imageops::FilterType::Triangle);
    } else if args.cover || candidate.dimensions() != (w, h) {
        candidate = imgdiff::cover(&candidate, w, h);
    }
    let mut reference = reference;
    let mut half = true;
    if let Some([x, y, cw, ch]) = args.crop {
        reference = imageops::crop_imm(&reference, x, y, cw, ch).to_image();
        candidate = imageops::crop_imm(&candidate, x, y, cw, ch).to_image();
        half = false;
    }
    let (w, h) = reference.dimensions();
    println!(
        "SSIM {:.4}  PSNR {:.2} dB  ({w}×{h})",
        imgdiff::ssim(&reference, &candidate),
        imgdiff::psnr(&reference, &candidate)
    );
    if let Some(path) = &args.diff {
        imgdiff::diff_image(&reference, &candidate).save(path)?;
    }
    if let Some(path) = &args.side_by_side {
        let (hw, hh) = if half { (w / 2, h / 2) } else { (w, h) };
        let mut out = RgbaImage::new(hw * 2, hh);
        let a = imageops::resize(&reference, hw, hh, imageops::FilterType::Triangle);
        let b = imageops::resize(&candidate, hw, hh, imageops::FilterType::Triangle);
        imageops::replace(&mut out, &a, 0, 0);
        imageops::replace(&mut out, &b, i64::from(hw), 0);
        out.save(path)?;
    }
    Ok(())
}
