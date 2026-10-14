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
    /// Write |reference − candidate| × 4 here.
    #[arg(long)]
    diff: Option<PathBuf>,
    /// Write reference | candidate side by side (half size) here.
    #[arg(long)]
    side_by_side: Option<PathBuf>,
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
    let (w, h) = reference.dimensions();
    if args.cover || candidate.dimensions() != (w, h) {
        candidate = imgdiff::cover(&candidate, w, h);
    }
    println!(
        "SSIM {:.4}  PSNR {:.2} dB  ({w}×{h})",
        imgdiff::ssim(&reference, &candidate),
        imgdiff::psnr(&reference, &candidate)
    );
    if let Some(path) = &args.diff {
        imgdiff::diff_image(&reference, &candidate).save(path)?;
    }
    if let Some(path) = &args.side_by_side {
        let (hw, hh) = (w / 2, h / 2);
        let mut out = RgbaImage::new(hw * 2, hh);
        let a = imageops::resize(&reference, hw, hh, imageops::FilterType::Triangle);
        let b = imageops::resize(&candidate, hw, hh, imageops::FilterType::Triangle);
        imageops::replace(&mut out, &a, 0, 0);
        imageops::replace(&mut out, &b, i64::from(hw), 0);
        out.save(path)?;
    }
    Ok(())
}
