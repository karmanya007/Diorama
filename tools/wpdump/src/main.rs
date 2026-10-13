//! `wpdump` — inspect and unpack Wallpaper Engine content.
//!
//! Output directories may contain copyrighted Workshop content; keep them out
//! of the repository (`/extracted/` is git-ignored).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use lumen_core::Limits;
use lumen_we::Pkg;

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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let limits = Limits::default();

    match cli.command {
        Command::List { pkg } => {
            let bytes = read(&pkg)?;
            let archive = Pkg::parse(&bytes, &limits)?;
            println!(
                "{} — {} entries",
                archive.version(),
                archive.entries().len()
            );
            for entry in archive.entries() {
                println!("{:>12}  {}", entry.size, entry.path);
            }
        }
        Command::Unpack { pkg, out_dir } => {
            let bytes = read(&pkg)?;
            let archive = Pkg::parse(&bytes, &limits)?;
            unpack(&archive, &out_dir)?;
            println!(
                "extracted {} entries to {}",
                archive.entries().len(),
                out_dir.display()
            );
        }
        Command::Cat { pkg, entry } => {
            let bytes = read(&pkg)?;
            let archive = Pkg::parse(&bytes, &limits)?;
            let Some(data) = archive.get(&entry) else {
                bail!("no entry {entry:?} in {}", pkg.display());
            };
            std::io::Write::write_all(&mut std::io::stdout(), data)?;
        }
    }
    Ok(())
}

fn read(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).with_context(|| format!("reading {}", path.display()))
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
