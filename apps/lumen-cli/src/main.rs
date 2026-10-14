//! `lumen` — control the running Lumen daemon.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use lumen_ipc::{Fit, Layout, MediaKind, Request, Response};

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show what is playing and the state of each monitor.
    Status,
    /// Show an image or video.
    Set {
        path: PathBuf,
        #[arg(long, value_enum)]
        fit: Option<FitArg>,
        #[arg(long, value_enum)]
        layout: Option<LayoutArg>,
        /// Override detection by file extension.
        #[arg(long, value_enum)]
        kind: Option<KindArg>,
    },
    /// Video soundtrack volume, 0–100 (0 mutes and closes the audio device).
    Volume {
        #[arg(value_parser = clap::value_parser!(u8).range(0..=100))]
        percent: u8,
    },
    /// Pause every monitor until `lumen resume`.
    Pause,
    /// Undo `lumen pause` (automatic pause rules still apply).
    Resume,
    /// Stop the daemon.
    Quit,
    /// Start lumend automatically when you sign in (per user, no admin rights).
    Autostart {
        #[arg(value_enum)]
        action: AutostartAction,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AutostartAction {
    Enable,
    Disable,
    Status,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum FitArg {
    Fill,
    Fit,
    Stretch,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LayoutArg {
    PerOutput,
    Span,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum KindArg {
    Image,
    Video,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lumen: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Registers `lumend --background` from the same folder as this `lumen` executable.
fn autostart(action: AutostartAction) -> Result<()> {
    match action {
        AutostartAction::Enable => {
            let daemon = std::env::current_exe()?
                .with_file_name(format!("lumend{}", std::env::consts::EXE_SUFFIX));
            if !daemon.is_file() {
                bail!("{} not found next to lumen", daemon.display());
            }
            let command = format!("\"{}\" --background", daemon.display());
            lumen_platform::set_autostart(Some(&command))?;
            println!("autostart enabled: {command}");
        }
        AutostartAction::Disable => {
            lumen_platform::set_autostart(None)?;
            println!("autostart disabled");
        }
        AutostartAction::Status => match lumen_platform::autostart()? {
            Some(cmd) => println!("autostart enabled: {cmd}"),
            None => println!("autostart disabled"),
        },
    }
    Ok(())
}

fn run(cli: Cli) -> Result<()> {
    let request = match cli.command {
        Command::Status => Request::Status,
        Command::Set {
            path,
            fit,
            layout,
            kind,
        } => Request::Set {
            // The daemon resolves paths itself; make them absolute so its working directory doesn't matter.
            path: std::path::absolute(&path)?,
            kind: kind.map(|k| match k {
                KindArg::Image => MediaKind::Image,
                KindArg::Video => MediaKind::Video,
            }),
            fit: fit.map(|f| match f {
                FitArg::Fill => Fit::Fill,
                FitArg::Fit => Fit::Fit,
                FitArg::Stretch => Fit::Stretch,
            }),
            layout: layout.map(|l| match l {
                LayoutArg::PerOutput => Layout::PerOutput,
                LayoutArg::Span => Layout::Span,
            }),
        },
        Command::Volume { percent } => Request::Volume { percent },
        Command::Pause => Request::Pause,
        Command::Resume => Request::Resume,
        Command::Quit => Request::Quit,
        Command::Autostart { action } => return autostart(action),
    };

    match lumen_ipc::call(&request)? {
        Response::Ok => Ok(()),
        Response::Error { message } => bail!("{message}"),
        Response::Status(status) => {
            match &status.content {
                Some(c) => println!(
                    "playing  {:?} {} (fit {:?}, layout {:?})",
                    c.kind,
                    c.path.display(),
                    c.fit,
                    c.layout
                ),
                None => println!("playing  nothing (use `lumen set <file>`)"),
            }
            println!("volume   {}%", status.volume);
            if status.user_paused {
                println!("paused   by `lumen pause`");
            }
            println!("daemon   pid {} v{}", status.pid, status.daemon_version);
            for o in &status.outputs {
                println!(
                    "{} {:<14} {:>4}×{:<4} at ({:>5},{:>5}) {:>7.2} Hz  {:<8} {:<10} {}",
                    if o.primary { '*' } else { ' ' },
                    o.device,
                    o.width,
                    o.height,
                    o.x,
                    o.y,
                    o.refresh_hz,
                    if o.paused { "paused" } else { "playing" },
                    o.coverage,
                    o.name
                );
            }
            Ok(())
        }
    }
}
