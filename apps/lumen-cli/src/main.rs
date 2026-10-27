//! `lumen` — control the running Lumen daemon.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use lumen_ipc::{
    Action, Fit, Layout, MediaKind, Order, Playlist, Policy, Request, Response, Schedule,
};

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show what is playing and the state of each monitor.
    Status {
        /// Also print each monitor's id and wallpaper.
        #[arg(short, long)]
        verbose: bool,
    },
    /// Show a wallpaper (image, video, scene or web page).
    Set {
        path: PathBuf,
        #[arg(long, value_enum)]
        fit: Option<FitArg>,
        #[arg(long, value_enum)]
        layout: Option<LayoutArg>,
        /// Override detection by file extension.
        #[arg(long, value_enum)]
        kind: Option<KindArg>,
        /// Web wallpapers: let this one use the network (remembered).
        #[arg(long, conflicts_with = "deny_network")]
        allow_network: bool,
        /// Web wallpapers: take network access away again (remembered).
        #[arg(long)]
        deny_network: bool,
        /// Only on this monitor (device name such as `\\.\DISPLAY1`, or its
        /// id from `lumen status -v`); repeat for more. Default: all monitors.
        #[arg(long = "output", value_name = "MONITOR")]
        outputs: Vec<String>,
    },
    /// Show a wallpaper's properties, or change them (remembered; showing
    /// wallpapers update at once).
    Props {
        /// The wallpaper (folder or file).
        path: PathBuf,
        /// `name=value` to set; the value is JSON (`5`, `true`, `"text"`) or
        /// else taken as text.
        #[arg(value_name = "NAME=VALUE")]
        set: Vec<String>,
        /// Return this property to WE's choice or the project default; repeat for more.
        #[arg(long, value_name = "NAME")]
        reset: Vec<String>,
    },
    /// Video soundtrack volume, 0–100 (0 mutes and closes the audio device).
    Volume {
        #[arg(value_parser = clap::value_parser!(u8).range(0..=100))]
        percent: u8,
    },
    /// Manage playlists (wallpapers that take turns).
    Playlist {
        #[command(subcommand)]
        action: PlaylistAction,
    },
    /// Play a saved playlist.
    Play {
        name: String,
        /// Only on this monitor; repeat for more. Default: all monitors.
        #[arg(long = "output", value_name = "MONITOR")]
        outputs: Vec<String>,
    },
    /// Move playlists on to their next wallpaper.
    Next {
        /// Only the playlist on this monitor; repeat for more.
        #[arg(long = "output", value_name = "MONITOR")]
        outputs: Vec<String>,
    },
    /// Show or change when wallpapers pause and how fast scenes run
    /// (remembered). Presets: balanced, quality, gaming, battery.
    Policy {
        /// Start from this preset.
        preset: Option<String>,
        #[arg(long, value_enum)]
        on_fullscreen: Option<ActionArg>,
        #[arg(long, value_enum)]
        on_maximized: Option<ActionArg>,
        #[arg(long, value_enum)]
        on_battery: Option<ActionArg>,
        #[arg(long, value_enum)]
        on_lock: Option<ActionArg>,
        /// Frame rate cap for scenes.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=240))]
        fps: Option<u32>,
    },
    /// What showing each wallpaper has cost (measured while it played
    /// alone), heaviest first.
    Costs,
    /// Convert a video the GPU cannot decode into a copy it can (H.264 at
    /// the size your monitors need); the copy then plays instead. Progress
    /// shows in `lumen status`.
    Convert {
        /// The video (file or Workshop folder).
        path: PathBuf,
    },
    /// Pause every monitor until `lumen resume`.
    Pause,
    /// Undo `lumen pause` (automatic pause rules still apply).
    Resume,
    /// Stop the daemon.
    Quit,
    /// List your wallpapers: Wallpaper Engine Workshop items and projects
    /// (found through Steam) and any folders given.
    Library {
        /// Also look in this folder (projects, images, videos); repeat for more.
        #[arg(long, value_name = "DIR")]
        folder: Vec<PathBuf>,
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Start lumend automatically when you sign in (per user, no admin rights).
    Autostart {
        #[arg(value_enum)]
        action: AutostartAction,
    },
}

#[derive(Debug, Subcommand)]
enum PlaylistAction {
    /// List the saved playlists.
    List,
    /// Save a playlist (replacing one with the same name).
    Save {
        name: String,
        /// Wallpapers (folders or files), in order.
        #[arg(required = true)]
        items: Vec<PathBuf>,
        /// Minutes per wallpaper.
        #[arg(long, default_value_t = 30, conflicts_with = "at")]
        every: u32,
        /// Play in random order.
        #[arg(long, conflicts_with = "at")]
        shuffle: bool,
        /// Time of day (HH:MM) each wallpaper starts, one per wallpaper.
        #[arg(long, value_name = "HH:MM")]
        at: Vec<String>,
    },
    /// Delete a playlist.
    Delete { name: String },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ActionArg {
    Keep,
    Pause,
}

impl From<ActionArg> for Action {
    fn from(a: ActionArg) -> Self {
        match a {
            ActionArg::Keep => Action::Keep,
            ActionArg::Pause => Action::Pause,
        }
    }
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
    Scene,
    Web,
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

/// `HH:MM` → minutes after midnight.
fn minutes(time: &str) -> Result<u32> {
    let (h, m) = time
        .split_once(':')
        .with_context(|| format!("expected HH:MM, got {time}"))?;
    let (h, m): (u32, u32) = (h.trim().parse()?, m.trim().parse()?);
    anyhow::ensure!(h < 24 && m < 60, "no such time {time}");
    Ok(h * 60 + m)
}

fn print_policy(p: &Policy) {
    let word = |a: Action| if a == Action::Pause { "pause" } else { "keep" };
    println!("policy   {}", p.preset_name().unwrap_or("custom"));
    println!("  fullscreen app  {}", word(p.fullscreen));
    println!("  maximized       {}", word(p.maximized));
    println!("  on battery      {}", word(p.battery));
    println!("  locked          {}", word(p.locked));
    println!("  scene fps       {}", p.fps);
}

fn library(folders: Vec<PathBuf>, json: bool) -> Result<()> {
    let mut roots = lumen_platform::steam_dir()
        .map(|d| lumen_library::Roots::from_steam(&d))
        .unwrap_or_default();
    roots.folders = folders;
    let items = lumen_library::scan(&roots);
    if json {
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }
    for i in &items {
        println!(
            "{:<12} {:<11} {:>8.1} MB  {}  ({})",
            format!("{:?}", i.kind).to_lowercase(),
            format!("{:?}", i.rating).to_lowercase(),
            i.size as f64 / 1e6,
            i.title,
            i.path.display()
        );
    }
    eprintln!("{} wallpapers", items.len());
    Ok(())
}

/// `name=value` settings and names to reset as one change set.
fn changes(set: &[String], reset: &[String]) -> Result<serde_json::Map<String, serde_json::Value>> {
    let mut out = serde_json::Map::new();
    for item in set {
        let Some((name, value)) = item.split_once('=') else {
            bail!("expected NAME=VALUE, got {item}");
        };
        let value = serde_json::from_str(value)
            .unwrap_or_else(|_| serde_json::Value::String(value.to_owned()));
        out.insert(name.trim().to_owned(), value);
    }
    for name in reset {
        out.insert(name.clone(), serde_json::Value::Null);
    }
    Ok(out)
}

fn run(cli: Cli) -> Result<()> {
    let verbose = matches!(cli.command, Command::Status { verbose: true });
    let request = match cli.command {
        Command::Status { .. } => Request::Status,
        Command::Set {
            path,
            fit,
            layout,
            kind,
            allow_network,
            deny_network,
            outputs,
        } => Request::Set {
            outputs: (!outputs.is_empty()).then_some(outputs),
            network: match (allow_network, deny_network) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            },
            // The daemon resolves paths itself; make them absolute so its working directory doesn't matter.
            path: std::path::absolute(&path)?,
            kind: kind.map(|k| match k {
                KindArg::Image => MediaKind::Image,
                KindArg::Video => MediaKind::Video,
                KindArg::Scene => MediaKind::Scene,
                KindArg::Web => MediaKind::Web,
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
        Command::Props { path, set, reset } => {
            let path = std::path::absolute(&path)?;
            if set.is_empty() && reset.is_empty() {
                Request::Properties { path }
            } else {
                Request::SetProperties {
                    path,
                    properties: changes(&set, &reset)?,
                }
            }
        }
        Command::Volume { percent } => Request::Volume { percent },
        Command::Pause => Request::Pause,
        Command::Resume => Request::Resume,
        Command::Quit => Request::Quit,
        Command::Autostart { action } => return autostart(action),
        Command::Library { folder, json } => return library(folder, json),
        Command::Playlist { action } => match action {
            PlaylistAction::List => Request::Playlists,
            PlaylistAction::Delete { name } => Request::DeletePlaylist { name },
            PlaylistAction::Save {
                name,
                items,
                every,
                shuffle,
                at,
            } => {
                let items = items
                    .iter()
                    .map(std::path::absolute)
                    .collect::<std::io::Result<Vec<_>>>()?;
                let schedule = if at.is_empty() {
                    Schedule::Interval { minutes: every }
                } else {
                    if at.len() != items.len() {
                        bail!(
                            "give one --at time per wallpaper ({} wallpapers)",
                            items.len()
                        );
                    }
                    Schedule::TimeOfDay {
                        starts: at.iter().map(|t| minutes(t)).collect::<Result<_>>()?,
                    }
                };
                Request::SavePlaylist {
                    playlist: Playlist {
                        name,
                        items,
                        order: if shuffle {
                            Order::Shuffle
                        } else {
                            Order::Sequential
                        },
                        schedule,
                    },
                }
            }
        },
        Command::Play { name, outputs } => Request::Play {
            name,
            outputs: (!outputs.is_empty()).then_some(outputs),
        },
        Command::Costs => Request::Costs,
        Command::Convert { path } => Request::Convert {
            path: std::path::absolute(&path)?,
        },
        Command::Next { outputs } => Request::Next {
            outputs: (!outputs.is_empty()).then_some(outputs),
        },
        Command::Policy {
            preset,
            on_fullscreen,
            on_maximized,
            on_battery,
            on_lock,
            fps,
        } => {
            let Response::Status(status) = lumen_ipc::call(&Request::Status)? else {
                bail!("the daemon did not report its policy");
            };
            let changed = preset.is_some()
                || on_fullscreen.is_some()
                || on_maximized.is_some()
                || on_battery.is_some()
                || on_lock.is_some()
                || fps.is_some();
            if !changed {
                print_policy(&status.policy);
                return Ok(());
            }
            let base = match preset.as_deref() {
                Some(name) => Policy::preset(name).with_context(|| {
                    format!("no preset {name} (balanced, quality, gaming, battery)")
                })?,
                None => status.policy,
            };
            let policy = Policy {
                fullscreen: on_fullscreen.map_or(base.fullscreen, Action::from),
                maximized: on_maximized.map_or(base.maximized, Action::from),
                battery: on_battery.map_or(base.battery, Action::from),
                locked: on_lock.map_or(base.locked, Action::from),
                fps: fps.unwrap_or(base.fps),
            };
            print_policy(&policy);
            Request::SetPolicy { policy }
        }
    };

    match lumen_ipc::call(&request)? {
        Response::Ok => Ok(()),
        Response::Error { message } => bail!("{message}"),
        Response::Playlists { playlists } => {
            if playlists.is_empty() {
                println!("no playlists (`lumen playlist save <name> <wallpapers…>`)");
            }
            for p in &playlists {
                let how = match &p.schedule {
                    Schedule::Interval { minutes } => format!(
                        "every {minutes} min{}",
                        if p.order == Order::Shuffle {
                            ", shuffled"
                        } else {
                            ""
                        }
                    ),
                    Schedule::TimeOfDay { .. } => "by time of day".to_owned(),
                };
                println!("{}  ({} wallpapers, {how})", p.name, p.items.len());
                for (i, item) in p.items.iter().enumerate() {
                    let at = match &p.schedule {
                        Schedule::TimeOfDay { starts } => starts
                            .get(i)
                            .map(|m| format!("{:02}:{:02}  ", m / 60, m % 60))
                            .unwrap_or_default(),
                        Schedule::Interval { .. } => String::new(),
                    };
                    println!("    {at}{}", item.display());
                }
            }
            Ok(())
        }
        Response::Costs { mut costs } => {
            if costs.is_empty() {
                println!("nothing measured yet (costs are measured while a wallpaper plays alone)");
            }
            costs.sort_by(|a, b| (b.gpu + b.cpu).total_cmp(&(a.gpu + a.cpu)));
            for c in &costs {
                let fps = if c.fps > 0 {
                    format!(", {} fps", c.fps)
                } else {
                    String::new()
                };
                println!(
                    "{:5.1}% GPU {:5.1}% CPU {:5} MB video memory {:5} MB memory  {}  ({:.1} MP{fps}, {} min)",
                    c.gpu,
                    c.cpu,
                    c.vram_mb,
                    c.ram_mb,
                    c.path.display(),
                    c.pixels as f64 / 1e6,
                    c.seconds / 60
                );
            }
            Ok(())
        }
        Response::Properties { values } => {
            for (name, value) in &values {
                println!("{name} = {value}");
            }
            Ok(())
        }
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
            if let Some(c) = &status.content {
                if c.converted {
                    println!("         (its copy converted for hardware decoding)");
                } else if c.software_decode {
                    println!(
                        "         decoded by the CPU: `lumen convert` makes a copy the GPU decodes"
                    );
                }
                if c.stuck {
                    println!("         the page stopped responding");
                }
                if !c.network && !c.blocked.is_empty() {
                    println!(
                        "         could not load {} (network off: `lumen set \"{}\" --allow-network`)",
                        c.blocked.join(", "),
                        c.path.display()
                    );
                }
            }
            for c in &status.conversions {
                match &c.error {
                    Some(e) => println!("convert  {} failed: {e}", c.path.display()),
                    None => println!("convert  {} {:.0}%", c.path.display(), c.progress * 100.0),
                }
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
                if verbose {
                    println!("    id   {}", o.id);
                }
                if let Some(p) = &o.playlist {
                    println!("    playlist {p}");
                }
                // Per monitor when monitors differ (or on request).
                if (verbose || status.outputs.iter().any(|x| x.content != status.content))
                    && let Some(c) = &o.content
                {
                    println!("    shows {:?} {}", c.kind, c.path.display());
                } else if o.content.is_none() {
                    println!("    shows nothing");
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_changes_parse_json_or_text() {
        let c = changes(
            &[
                "speed=5".into(),
                "on=true".into(),
                "title=hello world".into(),
                "color=\"1 0 0\"".into(),
            ],
            &["old".into()],
        )
        .unwrap();
        assert_eq!(c["speed"], 5);
        assert_eq!(c["on"], true);
        assert_eq!(c["title"], "hello world");
        assert_eq!(c["color"], "1 0 0");
        assert!(c["old"].is_null());
        assert!(changes(&["nope".into()], &[]).is_err());
    }

    #[test]
    fn times_of_day() {
        assert_eq!(minutes("07:30").unwrap(), 450);
        assert_eq!(minutes("0:00").unwrap(), 0);
        assert!(minutes("24:00").is_err());
        assert!(minutes("noon").is_err());
    }
}
