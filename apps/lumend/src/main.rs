//! `lumend` — Lumen wallpaper daemon.
//!
//! Shows an image or a looping video behind the desktop icons on every
//! monitor, controlled by the `lumen` CLI over a per-user local socket.
//! Static images render once (≈0% GPU idle, P2); video frames are presented on
//! their timestamps. Outputs pause individually when covered (P5/P6). Windows
//! and surfaces are rebuilt when Explorer restarts (P9), monitors change (P8)
//! or the GPU is lost (P11). Video never opens an audio device (P10).

mod clock;
mod convert;
mod costs;
mod headless;
mod media;
mod playlist;
mod policy;
mod properties;
mod scripts;
mod session;
mod slots;
mod sounds;
mod state;
mod tray;
mod videos;
mod web;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use lumen_gpu::{Backend, Gpu, GpuOptions};
use lumen_ipc::MediaKind;
use lumen_ipc::{ContentStatus, Fit, Layout, OutputStatus, Request, Response, Status};
use lumen_media::AudioPlayer;
use lumen_platform::{Coverage, DesktopEvent, Platform, TrayCommand, TrayState};

use crate::clock::{SceneClock, VideoClock};
use crate::policy::{Conditions, update_pauses};
use crate::session::{Content, Look, Session, Wallpaper};
use lumen_ipc::{Action, Playlist, Policy, Schedule};

#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Image to show at start-up (PNG, JPEG, …). Optional: `lumen set` can choose later.
    #[arg(long, conflicts_with_all = ["video", "scene", "web"])]
    image: Option<PathBuf>,
    /// Video to loop at start-up (MP4, …). Muted.
    #[arg(long, conflicts_with_all = ["scene", "web"])]
    video: Option<PathBuf>,
    /// Wallpaper Engine scene project folder (contains `scene.pkg` or `scene.json`).
    #[arg(long, conflicts_with = "web")]
    scene: Option<PathBuf>,
    /// Web wallpaper at start-up: a Wallpaper Engine web project folder or an HTML page.
    #[arg(long)]
    web: Option<PathBuf>,
    /// Wallpaper Engine `assets` folder (built-in shaders and effects used by scenes).
    #[arg(long, default_value = headless::DEFAULT_WE_ASSETS)]
    we_assets: PathBuf,
    /// Render one frame of `--scene` offscreen to this PNG and exit.
    #[arg(long, requires = "scene")]
    frame_out: Option<PathBuf>,
    /// Frame rate cap for scenes (default: the saved policy's, else 30).
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=240))]
    fps: Option<u32>,
    /// Scene time in seconds for `--frame-out`.
    #[arg(long, default_value_t = 0.0)]
    time: f32,
    /// With `--frame-out`: wall clock scripts see (`Date`, `engine.timeOfDay`),
    /// in milliseconds since the Unix epoch. Default: now.
    #[arg(long, requires = "frame_out")]
    clock: Option<f64>,
    /// With `--frame-out`: cursor position as fractions of the output (x right, y down).
    #[arg(long, value_parser = parse_pointer, default_value = "0.5,0.5")]
    pointer: [f32; 2],
    /// With `--frame-out`: present into a frame of this size (e.g. `3000x1920`)
    /// with the wallpaper's fit, instead of saving the raw scene.
    #[arg(long, value_parser = parse_size, requires = "frame_out")]
    frame_size: Option<[u32; 2]>,
    /// Wallpaper Engine `config.json` to read per-wallpaper fit from
    /// (default: the one next to `--we-assets`).
    #[arg(long)]
    we_config: Option<PathBuf>,
    /// With `--frame-out`: also save every intermediate render target beside it.
    #[arg(long, requires = "frame_out")]
    dump_targets: bool,
    /// Default: the wallpaper's own Wallpaper Engine setting if it has one, else fill.
    #[arg(long, value_enum)]
    fit: Option<FitArg>,
    /// `per-output`: the whole picture on every monitor. `span`: one picture across all monitors.
    #[arg(long, value_enum, default_value = "per-output")]
    layout: LayoutArg,
    #[arg(long, value_enum, default_value = "auto")]
    backend: BackendArg,
    /// Prefer an integrated GPU.
    #[arg(long)]
    low_power: bool,
    /// Let web wallpapers reach the network (off by default: only the
    /// wallpaper's own files load).
    #[arg(long)]
    web_network: bool,
    /// Exit after this many seconds (for testing).
    #[arg(long)]
    duration: Option<u64>,
    /// Print detected outputs and exit.
    #[arg(long)]
    list_outputs: bool,
    /// When a fullscreen app covers a monitor (games, video players).
    /// Default: the saved policy (`lumen policy`), else pause.
    #[arg(long, value_enum)]
    on_fullscreen: Option<ActionArg>,
    /// When a maximized window covers a monitor (default: saved, else pause).
    #[arg(long, value_enum)]
    on_maximized: Option<ActionArg>,
    /// While running on battery (default: saved, else keep).
    #[arg(long, value_enum)]
    on_battery: Option<ActionArg>,
    /// While the session is locked (default: saved, else pause).
    #[arg(long, value_enum)]
    on_lock: Option<ActionArg>,
    /// Run without a console (used by autostart); logs go to `lumend.log` in the config folder.
    #[arg(long)]
    background: bool,
    /// Also set each new wallpaper (a snapshot, for videos) as the Windows desktop background,
    /// so the desktop is not blank when Lumen is not running (P14). Changes your OS wallpaper setting.
    #[arg(long)]
    os_fallback: bool,
    /// Video soundtrack volume 0–100 (default: last used, else 0 = muted; no audio device opened).
    #[arg(long, value_parser = clap::value_parser!(u8).range(0..=100))]
    volume: Option<u8>,
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
enum BackendArg {
    Auto,
    Dx12,
    Vulkan,
}

fn parse_size(s: &str) -> std::result::Result<[u32; 2], String> {
    let (w, h) = s.split_once(['x', 'X']).ok_or("expected WxH")?;
    let parse = |v: &str| v.trim().parse::<u32>().map_err(|e| e.to_string());
    Ok([parse(w)?.max(1), parse(h)?.max(1)])
}

/// Same file or folder, ignoring case and separators (Windows paths).
fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// The fit Wallpaper Engine uses for `wallpaper` on the first monitor.
fn we_fit(config: &std::path::Path, wallpaper: &std::path::Path) -> Option<Fit> {
    use lumen_we::config::{Alignment, alignment_from_file};
    Some(match alignment_from_file(config, wallpaper, 0)? {
        Alignment::Stretch => Fit::Stretch,
        Alignment::Native { x, y } => Fit::Native {
            x: x.round() as i32,
            y: y.round() as i32,
        },
        Alignment::Custom { x, y, zoom } => Fit::Custom {
            x: x.round() as i32,
            y: y.round() as i32,
            zoom: zoom.round() as i32,
        },
        Alignment::Cover | Alignment::Other(_) => Fit::Fill,
    })
}

fn parse_pointer(s: &str) -> std::result::Result<[f32; 2], String> {
    let (x, y) = s.split_once(',').ok_or("expected x,y")?;
    let parse = |v: &str| v.trim().parse::<f32>().map_err(|e| e.to_string());
    Ok([parse(x)?, parse(y)?])
}

fn we_config_path(args: &Args) -> Option<PathBuf> {
    args.we_config
        .clone()
        .or_else(|| args.we_assets.parent().map(|p| p.join("config.json")))
        .filter(|p| p.is_file())
}

fn fit_mode(fit: Fit) -> lumen_render::FitMode {
    use lumen_render::FitMode;
    match fit {
        Fit::Fill => FitMode::Fill,
        Fit::Fit => FitMode::Fit,
        Fit::Stretch => FitMode::Stretch,
        Fit::Custom { x, y, zoom } => FitMode::Custom { x, y, zoom },
        Fit::Native { x, y } => FitMode::Native { x, y },
    }
}

fn gpu_options(args: &Args) -> GpuOptions {
    GpuOptions {
        backend: match args.backend {
            BackendArg::Auto => Backend::Auto,
            BackendArg::Dx12 => Backend::Dx12,
            BackendArg::Vulkan => Backend::Vulkan,
        },
        low_power: args.low_power,
    }
}

const COVERAGE_INTERVAL: Duration = Duration::from_millis(500);
const BATTERY_INTERVAL: Duration = Duration::from_secs(5);
const REBUILD_RETRY: Duration = Duration::from_secs(1);
/// After device loss, how long to wait for the same GPU before settling for
/// another one: during a TDR the discrete GPU disappears for a moment and the
/// integrated one would otherwise take over for good.
const SAME_ADAPTER_GRACE: Duration = Duration::from_secs(10);
const AUDIO_CHECK_INTERVAL: Duration = Duration::from_secs(2);

type Job = (Request, SyncSender<Response>);

fn main() -> Result<()> {
    let args = Args::parse();
    let config_dir = lumen_platform::config_dir();
    init_logging(args.background, config_dir.as_deref());
    let platform = Platform::new().context("platform init")?;
    // DXC runs per new shader variant; no console window may flash.
    lumen_gpu::shader::set_process_hook(lumen_platform::no_console);

    if let (Some(scene), Some(out)) = (&args.scene, &args.frame_out) {
        let assets = args.we_assets.is_dir().then_some(args.we_assets.as_path());
        let request = headless::FrameRequest {
            time: args.time,
            clock_ms: args.clock.unwrap_or_else(scripts::now_ms),
            pointer: args.pointer,
            out,
            dump_targets: args.dump_targets,
            present: args.frame_size.map(|size| {
                let fit = match args.fit {
                    Some(FitArg::Fill) => Fit::Fill,
                    Some(FitArg::Fit) => Fit::Fit,
                    Some(FitArg::Stretch) => Fit::Stretch,
                    None => we_config_path(&args)
                        .and_then(|c| we_fit(&c, scene))
                        .unwrap_or(Fit::Fill),
                };
                (size, fit_mode(fit))
            }),
        };
        let we_config = we_config_path(&args);
        return headless::render_to_png(
            scene,
            assets,
            we_config.as_deref(),
            gpu_options(&args),
            &request,
        );
    }

    if args.list_outputs {
        for o in platform.outputs()? {
            println!(
                "{}{} {}×{} at ({}, {}) {:.2} Hz {} {} [{}]",
                if o.primary { "* " } else { "  " },
                o.device,
                o.width,
                o.height,
                o.x,
                o.y,
                f64::from(o.refresh_mhz) / 1000.0,
                o.manufacturer,
                o.name,
                o.id.0
            );
        }
        return Ok(());
    }

    let server = lumen_ipc::Server::bind().context("starting control socket")?;
    let (jobs_tx, jobs) = sync_channel::<Job>(16);
    let waker = platform.waker();
    std::thread::Builder::new()
        .name("ipc".into())
        .spawn(move || {
            server.serve(|request| {
                let (reply_tx, reply_rx) = sync_channel(1);
                if jobs_tx.send((request, reply_tx)).is_err() {
                    return Response::Error {
                        message: "daemon is shutting down".into(),
                    };
                }
                waker.wake();
                reply_rx
                    .recv_timeout(Duration::from_secs(30))
                    .unwrap_or_else(|_| Response::Error {
                        message: "daemon did not answer".into(),
                    })
            });
        })
        .context("starting IPC thread")?;

    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst)).context("Ctrl+C handler")?;
    let deadline = args
        .duration
        .map(|s| Instant::now() + Duration::from_secs(s));

    let gpu_options = gpu_options(&args);
    // Games and foreground apps win CPU and GPU scheduling (P5).
    platform.lower_priority();

    let mut daemon = Daemon {
        platform: &platform,
        gpu: Some(Gpu::new(gpu_options).context("GPU init")?),
        gpu_options,
        policy: Policy::default(),
        conditions: Conditions {
            on_battery: platform.on_battery(),
            ..Conditions::default()
        },
        slots: Vec::new(),
        config_dir: config_dir.clone(),
        os_fallback: args.os_fallback,
        volume: 0,
        next_audio_check: Instant::now() + AUDIO_CHECK_INTERVAL,
        web_network: args.web_network,
        web_allowed: Vec::new(),
        overrides: Vec::new(),
        playlists: Vec::new(),
        rng: playlist::Rng::from_time(),
        folders: Vec::new(),
        retiring: Vec::new(),
        fps: Policy::default().fps,
        we_config: we_config_path(&args),
        we_assets: args.we_assets.is_dir().then(|| args.we_assets.clone()),
        next_coverage: Instant::now(),
        next_battery: Instant::now() + BATTERY_INTERVAL,
        needs_rebuild: false,
        reset_gpu: false,
        lost_adapter: None,
        quit: false,
        costs: costs::Costs::load(lumen_platform::cache_dir().as_deref()),
        conversions: convert::Conversions::default(),
    };

    let start_path = args
        .image
        .as_ref()
        .or(args.video.as_ref())
        .or(args.scene.as_ref())
        .or(args.web.as_ref());
    let look = Look {
        fit: match args.fit {
            Some(FitArg::Fill) => Fit::Fill,
            Some(FitArg::Fit) => Fit::Fit,
            Some(FitArg::Stretch) => Fit::Stretch,
            None => start_path.map_or(Fit::Fill, |p| daemon.default_fit(p)),
        },
        layout: match args.layout {
            LayoutArg::PerOutput => Layout::PerOutput,
            LayoutArg::Span => Layout::Span,
        },
    };
    // Start-up content: command line, else the wallpaper from last time.
    let saved_state = config_dir.as_deref().map(state::load).unwrap_or_default();
    daemon.volume = args.volume.unwrap_or(saved_state.volume).min(100);
    daemon.web_allowed = saved_state.web_network.clone();
    // The saved policy, with what the command line names on top.
    let saved_policy = saved_state.policy.unwrap_or_default();
    daemon.policy = Policy {
        fullscreen: args
            .on_fullscreen
            .map_or(saved_policy.fullscreen, Action::from),
        maximized: args
            .on_maximized
            .map_or(saved_policy.maximized, Action::from),
        battery: args.on_battery.map_or(saved_policy.battery, Action::from),
        locked: args.on_lock.map_or(saved_policy.locked, Action::from),
        fps: args.fps.unwrap_or(saved_policy.fps),
    };
    daemon.fps = daemon.policy.fps;
    daemon.playlists = saved_state.playlists.clone();
    daemon.folders = saved_state.folders.clone();
    daemon.overrides = saved_state
        .properties
        .iter()
        .map(|p| (p.path.clone(), p.values.clone()))
        .collect();
    if let Some(path) = start_path {
        match daemon.load(path, None, look) {
            Ok(w) => {
                if let Err(e) = daemon.set_wallpaper(w, Vec::new()) {
                    tracing::warn!("{e:#}");
                }
            }
            Err(e) => tracing::error!("start-up wallpaper: {e:#}"),
        }
    } else if saved_state.wallpapers.is_empty() {
        tracing::info!("no wallpaper yet; waiting for `lumen set <file>`");
    } else {
        // Last time's wallpapers, each on its monitors.
        let mut slots = Vec::new();
        for s in saved_state.wallpapers {
            let look = Look {
                fit: s.fit,
                layout: s.layout,
            };
            match daemon.load(&s.path, Some(s.kind), look) {
                Ok(w) => {
                    let mut slot = Slot::new(w, s.outputs, daemon.fps);
                    // Playlists carry on from where they were.
                    slot.run = s.playlist.and_then(|r| {
                        let list = daemon.playlists.iter().find(|p| p.name == r.name)?;
                        Some(Run::new(
                            list,
                            r.index.min(list.items.len().saturating_sub(1)),
                        ))
                    });
                    slots.push(slot);
                }
                Err(e) => tracing::error!("start-up wallpaper {}: {e:#}", s.path.display()),
            }
        }
        daemon.slots = slots;
        daemon.rebuild(true);
        let volume = daemon.volume;
        for slot in &mut daemon.slots {
            slot.start_audio(volume);
        }
    }

    // Notification-area icon: open the UI, next, pause/resume, quit.
    let size = platform.tray_icon_size();
    let shown = tray::icon(size).and_then(|rgba| {
        platform
            .show_tray(&rgba, size, daemon.tray_state())
            .context("tray icon")
    });
    if let Err(e) = shown {
        tracing::warn!("{e:#}");
    }

    while !stop.load(Ordering::SeqCst)
        && !daemon.quit
        && deadline.is_none_or(|d| Instant::now() < d)
    {
        daemon.tick(&jobs)?;
    }
    daemon.costs.save();
    drop(daemon);
    tracing::info!("exiting");
    Ok(())
}

fn set_fallback(image: &std::path::Path, span: bool) {
    match lumen_platform::set_os_wallpaper(image, span) {
        Ok(()) => tracing::info!(path = %image.display(), "OS fallback wallpaper set"),
        Err(e) => tracing::warn!("could not set OS fallback wallpaper: {e}"),
    }
}

/// Saves the current scene frame as a PNG and makes it the OS wallpaper.
fn scene_fallback(dir: Option<&std::path::Path>, gpu: &Gpu, session: &Session, span: bool) {
    let Some(dir) = dir else {
        return;
    };
    let path = dir.join("fallback.png");
    match session.scene_snapshot(gpu).map(|img| img.save(&path)) {
        Some(Ok(())) => set_fallback(&path, span),
        Some(Err(e)) => tracing::warn!("fallback snapshot not saved: {e}"),
        None => tracing::warn!("no scene frame for the fallback snapshot"),
    }
}

/// Running without a console (`--background`).
static BACKGROUND: AtomicBool = AtomicBool::new(false);
/// In the background: where web hosts write their log.
static WEBHOST_LOG: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Where web hosts write their log: lumend's own stderr, except in the
/// background, where that is no valid handle (inheriting it makes starting
/// the child fail with "The request is not supported", os error 50): then
/// `<config dir>/lumen-webhost.log`.
fn child_stderr() -> std::process::Stdio {
    if !BACKGROUND.load(Ordering::Relaxed) {
        return std::process::Stdio::inherit();
    }
    WEBHOST_LOG
        .get()
        .and_then(|path| {
            // Started afresh once it passes 1 MB.
            if std::fs::metadata(path).is_ok_and(|m| m.len() > 1 << 20) {
                let _ = std::fs::remove_file(path);
            }
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
        })
        .map_or_else(std::process::Stdio::null, std::process::Stdio::from)
}

/// Console logging normally; with `--background`, detach from the console and
/// append to `<config dir>/lumend.log` instead.
fn init_logging(background: bool, config_dir: Option<&std::path::Path>) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let file = background.then_some(config_dir).flatten().and_then(|dir| {
        std::fs::create_dir_all(dir).ok()?;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("lumend.log"))
            .ok()
    });
    if background {
        lumen_platform::detach_console();
        BACKGROUND.store(true, Ordering::Relaxed);
        if let Some(dir) = config_dir {
            let _ = WEBHOST_LOG.set(dir.join("lumen-webhost.log"));
        }
    }
    match file {
        Some(f) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(f))
            .init(),
        None => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }
}

/// Identifies a physical GPU across device re-creation.
type AdapterKey = (u32, u32, lumen_gpu::wgpu::Backend);

fn adapter_key(gpu: &Gpu) -> AdapterKey {
    let info = gpu.adapter.get_info();
    (info.vendor, info.device, info.backend)
}

/// One wallpaper and the monitors it shows on.
struct Slot {
    /// Output ids it claims; empty: every monitor no other wallpaper claims.
    claim: Vec<String>,
    wallpaper: Wallpaper,
    /// `None` while it shows nowhere (its monitors are unplugged) or between rebuilds.
    session: Option<Session>,
    /// Output ids the session was built for.
    shown_on: Vec<String>,
    /// Shows on the primary monitor: its soundtrack and sounds play and it
    /// makes the OS fallback, so sounds never overlap.
    primary: bool,
    clock: VideoClock,
    scene_clock: SceneClock,
    /// Last cursor position over the scene (kept while it is elsewhere).
    pointer: [f32; 2],
    audio: Option<AudioPlayer>,
    /// Video loop index the audio was last synced to; `None` = not started yet.
    audio_loop: Option<u32>,
    /// Fallback snapshot still to be taken from the next shown frame.
    fallback_pending: bool,
    /// The playlist this wallpaper came from.
    run: Option<Run>,
    /// Failed attempts to show it since it last showed (see [`MAX_TRIES`]).
    failures: u32,
    /// Why it last failed to show.
    error: Option<String>,
    /// Its session has drawn and its windows are on top (until then a
    /// replaced wallpaper stays visible above it).
    shown: bool,
}

/// How long a replaced wallpaper may stay up while the new one gets ready.
const RETIRE_AFTER: Duration = Duration::from_secs(6);

/// Keeps a replaced wallpaper's windows on screen, silent and frozen, until
/// the new one has drawn (see `Daemon::retiring`).
fn retire(retiring: &mut Vec<(Session, Instant)>, mut session: Session) {
    if let Some(sounds) = session.scene_sounds() {
        sounds.set_master(0.0);
    }
    retiring.push((session, Instant::now() + RETIRE_AFTER));
}

/// Attempts to show a wallpaper before giving up until the desktop changes
/// (a missing shader compiler or a broken scene does not fix itself).
const MAX_TRIES: u32 = 3;

/// A playlist playing on a slot.
struct Run {
    name: String,
    /// Position of the wallpaper showing.
    index: usize,
    /// Time-of-day playlists: the item the clock last chose (`Next` may have moved on since).
    scheduled: Option<usize>,
    /// When to move on (interval) or look at the clock again (time of day).
    next: Instant,
}

/// How often time-of-day playlists look at the clock.
const CLOCK_CHECK: Duration = Duration::from_secs(30);

impl Run {
    fn new(playlist: &Playlist, index: usize) -> Self {
        let scheduled = match &playlist.schedule {
            Schedule::TimeOfDay { starts } => playlist::by_time(starts, playlist::minute_of_day()),
            Schedule::Interval { .. } => None,
        };
        Self {
            name: playlist.name.clone(),
            index,
            scheduled,
            next: Instant::now() + Self::wait(playlist),
        }
    }

    fn wait(playlist: &Playlist) -> Duration {
        match playlist.schedule {
            Schedule::Interval { minutes } => Duration::from_secs(60 * u64::from(minutes.max(1))),
            Schedule::TimeOfDay { .. } => CLOCK_CHECK,
        }
    }
}

/// What a slot's frame asks of the daemon.
enum Flow {
    Continue,
    /// Windows or the web host went away: rebuild.
    Rebuild,
    /// A frame failed: recreate the GPU device.
    ResetGpu,
}

impl Slot {
    fn new(wallpaper: Wallpaper, claim: Vec<String>, fps: u32) -> Self {
        Self {
            claim,
            wallpaper,
            session: None,
            shown_on: Vec::new(),
            primary: false,
            clock: VideoClock::default(),
            scene_clock: SceneClock::new(fps),
            pointer: [0.5, 0.5],
            audio: None,
            audio_loop: None,
            fallback_pending: false,
            run: None,
            failures: 0,
            error: None,
            shown: false,
        }
    }

    fn saved(&self) -> state::SavedWallpaper {
        state::SavedWallpaper {
            path: self.wallpaper.path.clone(),
            kind: self.wallpaper.kind,
            fit: self.wallpaper.look.fit,
            layout: self.wallpaper.look.layout,
            outputs: self.claim.clone(),
            playlist: self.run.as_ref().map(|r| state::SavedRun {
                name: r.name.clone(),
                index: r.index,
            }),
        }
    }

    fn content_status(&self) -> ContentStatus {
        ContentStatus {
            kind: self.wallpaper.kind,
            path: self.wallpaper.path.clone(),
            fit: self.wallpaper.look.fit,
            layout: self.wallpaper.look.layout,
            software_decode: self.wallpaper.decode.software,
            converted: self.wallpaper.decode.converted,
        }
    }

    /// Time until this slot next needs the loop.
    fn wait(&mut self) -> Duration {
        match &self.wallpaper.content {
            _ if self.session.is_none() => Duration::from_secs(1),
            Content::Video(_) if self.clock.is_paused() => COVERAGE_INTERVAL,
            Content::Video(player) => self.clock.time_until_next(player),
            Content::Scene(_) if self.scene_clock.is_paused() => COVERAGE_INTERVAL,
            Content::Scene(_) => self.scene_clock.time_until_next(),
            Content::Web(_) => self
                .session
                .as_mut()
                .and_then(Session::web)
                .map_or(Duration::from_secs(1), |w| w.time_until_next()),
            Content::Image(_) => Duration::from_secs(1),
        }
    }

    /// (Re)creates the soundtrack player for a video on the primary monitor,
    /// if volume > 0. Errors only mean silence; video is unaffected (P10).
    fn start_audio(&mut self, volume: u8) {
        self.audio = None;
        self.audio_loop = None;
        if volume == 0 || !self.primary || self.wallpaper.kind != MediaKind::Video {
            return;
        }
        match AudioPlayer::spawn(&self.wallpaper.path, f32::from(volume) / 100.0) {
            Ok(a) => self.audio = Some(a),
            Err(e) => tracing::info!("no video audio: {e}"),
        }
    }

    /// Scene sounds play only on the primary monitor's wallpaper.
    fn set_sound_volume(&mut self, volume: u8) {
        let master = if self.primary {
            f32::from(volume) / 100.0
        } else {
            0.0
        };
        if let Some(sounds) = self.session.as_mut().and_then(Session::scene_sounds) {
            sounds.set_master(master);
        }
    }

    /// Pauses, web upkeep and due frames. `coverage`: re-check what covers the monitors.
    fn frame(
        &mut self,
        platform: &Platform,
        gpu: &Gpu,
        policy: Policy,
        conditions: Conditions,
        coverage: bool,
        config_dir: Option<&Path>,
    ) -> Result<Flow> {
        let Some(session) = self.session.as_mut() else {
            return Ok(Flow::Continue);
        };
        let span = self.wallpaper.look.layout == Layout::Span;
        if coverage {
            let resumed = update_pauses(platform, session, policy, conditions);
            let all_paused = session.outputs.iter().all(|o| o.paused);
            match &self.wallpaper.content {
                Content::Video(_) => {
                    self.clock.set_paused(all_paused);
                    if let (Some(audio), Some(_)) = (&self.audio, self.audio_loop) {
                        audio.set_paused(all_paused);
                    }
                    // Freshly resumed outputs still show an old frame: present the current one now.
                    if resumed && let Err(e) = session.present_all(gpu) {
                        tracing::warn!("present failed: {e:#}; resetting GPU");
                        return Ok(Flow::ResetGpu);
                    }
                }
                Content::Scene(_) => {
                    // Resuming makes the next frame due immediately.
                    self.scene_clock.set_paused(all_paused);
                    if let Some(sounds) = session.scene_sounds() {
                        sounds.set_paused(all_paused);
                    }
                }
                Content::Web(_) | Content::Image(_) => {}
            }
            let pauses: Vec<(String, bool)> = session
                .outputs
                .iter()
                .map(|o| (o.info.id.0.clone(), o.paused))
                .collect();
            if let Some(web) = session.web() {
                web.set_paused(pauses);
            }
        }

        if let Some(web) = session.web() {
            if !self.shown && web.ready() {
                self.shown = true;
            }
            if !web.alive() {
                tracing::warn!("web wallpaper host exited; restarting");
                return Ok(Flow::Rebuild);
            }
            web.tick(platform);
            if self.fallback_pending
                && let Some(dir) = config_dir
            {
                let path = dir.join("fallback.png");
                match web.snapshot(&path) {
                    web::Snapshot::Pending => {}
                    web::Snapshot::Ready => {
                        self.fallback_pending = false;
                        set_fallback(&path, span);
                    }
                    web::Snapshot::Failed => {
                        self.fallback_pending = false;
                        tracing::warn!("no web wallpaper snapshot for the OS fallback");
                    }
                }
            }
        }

        if let Content::Scene(_) = &self.wallpaper.content
            && self.scene_clock.frame_due()
        {
            let on_scene = platform.cursor_position().and_then(|c| session.pointer(c));
            if let Some(p) = on_scene {
                self.pointer = p;
            }
            // Cursor events only while the cursor is over the wallpaper.
            let buttons = on_scene.map(|_| platform.desktop_pointer());
            if let Err(e) =
                session.show_scene_frame(gpu, self.scene_clock.time(), self.pointer, buttons)
            {
                tracing::warn!("scene frame failed: {e:#}; resetting GPU");
                return Ok(Flow::ResetGpu);
            }
            if !self.shown {
                session.raise();
                self.shown = true;
            }
            if self.fallback_pending {
                self.fallback_pending = false;
                scene_fallback(config_dir, gpu, session, span);
            }
        }

        if let Content::Video(player) = &self.wallpaper.content
            && !self.clock.is_paused()
            && let Some(frame) = self.clock.due_frame(player)
        {
            let loop_index = frame.loop_index;
            if let Err(e) = session.show_video_frame(gpu, frame) {
                tracing::warn!("video frame failed: {e:#}; resetting GPU");
                return Ok(Flow::ResetGpu);
            }
            if !self.shown {
                session.raise();
                self.shown = true;
            }
            if self.fallback_pending {
                self.fallback_pending = false;
                self.video_fallback(config_dir);
            }
            // Audio starts with the first frame and restarts whenever the video wraps.
            if let Some(audio) = &self.audio {
                match self.audio_loop {
                    None => audio.set_paused(false),
                    Some(prev) if prev != loop_index => audio.restart(),
                    Some(_) => {}
                }
                self.audio_loop = Some(loop_index);
            }
        }
        Ok(Flow::Continue)
    }

    /// Saves the current video frame as a PNG and makes it the OS wallpaper.
    fn video_fallback(&self, config_dir: Option<&Path>) {
        let (Some(dir), Some(frame)) = (config_dir, self.clock.current.as_ref()) else {
            return;
        };
        let path = dir.join("fallback.png");
        let saved = image::RgbaImage::from_raw(frame.width, frame.height, frame.to_rgba())
            .map(|img| img.save(&path));
        match saved {
            Some(Ok(())) => set_fallback(&path, self.wallpaper.look.layout == Layout::Span),
            Some(Err(e)) => tracing::warn!("fallback snapshot not saved: {e}"),
            None => tracing::warn!("fallback snapshot has wrong size"),
        }
    }
}

struct Daemon<'p> {
    platform: &'p Platform,
    /// `None` between a device loss and a successful re-init.
    gpu: Option<Gpu>,
    gpu_options: GpuOptions,
    policy: Policy,
    conditions: Conditions,
    /// What plays where, in the order the user set them.
    slots: Vec<Slot>,
    fps: u32,
    /// Wallpaper Engine's `config.json` (per-wallpaper fit), if present.
    we_config: Option<PathBuf>,
    /// `<WE install>/assets` for scene built-ins, if present.
    we_assets: Option<PathBuf>,
    next_coverage: Instant,
    next_battery: Instant,
    /// A rebuild failed (e.g. Explorer still starting); retry on the next tick.
    needs_rebuild: bool,
    /// Recreate the GPU device: a frame failed (often the first sign of a
    /// TDR, before the device-lost callback runs) or re-init failed earlier.
    reset_gpu: bool,
    /// The GPU that was lost and when (for [`SAME_ADAPTER_GRACE`]).
    lost_adapter: Option<(AdapterKey, Instant)>,
    quit: bool,
    /// What each wallpaper costs to show, measured as it plays.
    costs: costs::Costs,
    /// Videos being converted for hardware decoding.
    conversions: convert::Conversions,
    config_dir: Option<PathBuf>,
    os_fallback: bool,
    /// Soundtrack volume 0–100; 0 means no audio player at all.
    volume: u8,
    next_audio_check: Instant,
    /// Every web wallpaper may use the network (`--web-network`).
    web_network: bool,
    /// Web wallpaper folders allowed to use the network (`lumen set
    /// --allow-network`), remembered in the state file.
    web_allowed: Vec<PathBuf>,
    /// Property values set in Lumen, per wallpaper path.
    overrides: Vec<(PathBuf, properties::Values)>,
    /// Named playlists.
    playlists: Vec<Playlist>,
    rng: playlist::Rng,
    /// Folders the library also lists (kept for the UI).
    folders: Vec<PathBuf>,
    /// Replaced wallpapers still on screen until their successors have drawn
    /// (no black gap when switching), each with a deadline.
    retiring: Vec<(Session, Instant)>,
}

impl Daemon<'_> {
    /// One iteration: wait for events/IPC/next frame, then react.
    fn tick(&mut self, jobs: &Receiver<Job>) -> Result<()> {
        let content_wait = self
            .slots
            .iter_mut()
            .map(Slot::wait)
            .min()
            .unwrap_or(Duration::from_secs(1));
        let wait = content_wait
            .min(self.next_coverage.saturating_duration_since(Instant::now()))
            .min(
                self.costs
                    .next_sample()
                    .saturating_duration_since(Instant::now()),
            );
        let events = self.platform.poll_events(wait);
        if let Some(gpu) = &self.gpu {
            gpu.maintain();
        }

        while let Ok((request, reply)) = jobs.try_recv() {
            let response = self.handle(request);
            let _ = reply.send(response);
        }

        for e in &events {
            match e {
                DesktopEvent::SessionLocked => self.conditions.locked = true,
                DesktopEvent::SessionUnlocked => self.conditions.locked = false,
                DesktopEvent::Tray(command) => self.on_tray(*command),
                _ => {}
            }
        }
        self.platform.set_tray_state(self.tray_state());
        if Instant::now() >= self.next_battery {
            self.conditions.on_battery = self.platform.on_battery();
            self.next_battery = Instant::now() + BATTERY_INTERVAL;
        }

        let device_lost = self.reset_gpu || self.gpu.as_ref().is_some_and(Gpu::is_lost);
        if device_lost {
            // Everything holding the old device goes first: D3D12 hands back the
            // same (removed) device for an adapter while one is still alive.
            self.retiring.clear();
            for slot in &mut self.slots {
                slot.session = None;
            }
            if let Some(old) = self.gpu.take() {
                self.lost_adapter = Some((adapter_key(&old), Instant::now()));
            }
            self.reset_gpu = false;
        }
        if self.gpu.is_none() {
            // The adapter can be unavailable for a few seconds during a TDR.
            match Gpu::new(self.gpu_options) {
                Ok(gpu)
                    if self.lost_adapter.is_some_and(|(key, since)| {
                        adapter_key(&gpu) != key && since.elapsed() < SAME_ADAPTER_GRACE
                    }) =>
                {
                    tracing::info!(
                        found = %gpu.adapter.get_info().name,
                        "previous GPU not back yet; waiting"
                    );
                    drop(gpu);
                    std::thread::sleep(REBUILD_RETRY);
                    return Ok(());
                }
                Ok(gpu) => {
                    self.gpu = Some(gpu);
                    self.lost_adapter = None;
                    self.needs_rebuild = true;
                }
                Err(e) => {
                    tracing::warn!("GPU re-init failed: {e:#}; retrying");
                    std::thread::sleep(REBUILD_RETRY);
                    return Ok(());
                }
            }
        }
        let desktop_changed = events.iter().any(|e| e.needs_rebuild());
        if desktop_changed || self.needs_rebuild {
            tracing::info!(?events, device_lost, "rebuilding wallpaper");
            // A changed desktop rebuilds everything; otherwise only what is missing.
            self.rebuild(desktop_changed);
            return Ok(());
        }

        let Some(gpu) = self.gpu.as_ref() else {
            return Ok(());
        };
        let coverage =
            Instant::now() >= self.next_coverage || events.iter().any(|e| !e.needs_rebuild());
        if coverage {
            self.next_coverage = Instant::now() + COVERAGE_INTERVAL;
        }
        for slot in &mut self.slots {
            let flow = slot.frame(
                self.platform,
                gpu,
                self.policy,
                self.conditions,
                coverage,
                self.config_dir.as_deref(),
            )?;
            match flow {
                Flow::Continue => {}
                Flow::Rebuild => {
                    self.needs_rebuild = true;
                    std::thread::sleep(REBUILD_RETRY);
                    return Ok(());
                }
                Flow::ResetGpu => {
                    self.reset_gpu = true;
                    return Ok(());
                }
            }
        }

        let showing = self.showing_alone();
        let hosts: Vec<u32> = self
            .slots
            .iter()
            .filter_map(|s| s.session.as_ref()?.web_pid())
            .collect();
        self.costs.sample(showing, &[std::process::id()], &hosts);

        // Replaced wallpapers go once every new one has drawn (or in time).
        if !self.retiring.is_empty() {
            let all_shown = self.slots.iter().all(|s| s.session.is_none() || s.shown);
            let now = Instant::now();
            self.retiring
                .retain(|(_, until)| !all_shown && now < *until);
        }

        self.advance_playlists(None);

        // Converted videos replace their originals where they show.
        for video in self.conversions.finished() {
            self.reload_video(&video);
        }

        if Instant::now() >= self.next_audio_check {
            self.next_audio_check = Instant::now() + AUDIO_CHECK_INTERVAL;
            for slot in &mut self.slots {
                if slot.audio.as_ref().is_some_and(AudioPlayer::needs_reopen) {
                    tracing::info!("audio device changed; reopening");
                    slot.start_audio(self.volume);
                }
                if let Some(sounds) = slot.session.as_mut().and_then(Session::scene_sounds) {
                    sounds.check_devices();
                }
            }
        }
        Ok(())
    }

    /// Moves playlists on: those whose time has come, or with `skip` those
    /// on these monitors (all when empty).
    fn advance_playlists(&mut self, skip: Option<&[String]>) {
        let now = Instant::now();
        let minute = playlist::minute_of_day();
        let mut moves = Vec::new();
        for (i, slot) in self.slots.iter_mut().enumerate() {
            let Some(run) = slot.run.as_mut() else {
                continue;
            };
            let Some(list) = self.playlists.iter().find(|p| p.name == run.name) else {
                continue;
            };
            let target = match (skip, &list.schedule) {
                (Some(outputs), _) => (outputs.is_empty()
                    || slot.shown_on.iter().any(|o| outputs.contains(o)))
                .then(|| playlist::after(list, run.index, &mut self.rng)),
                (None, _) if now < run.next => None,
                (None, Schedule::Interval { .. }) => {
                    Some(playlist::after(list, run.index, &mut self.rng))
                }
                (None, Schedule::TimeOfDay { starts }) => {
                    run.next = now + CLOCK_CHECK;
                    let chosen = playlist::by_time(starts, minute);
                    let changed = chosen != run.scheduled;
                    run.scheduled = chosen;
                    chosen.filter(|_| changed)
                }
            };
            if let Some(index) = target {
                moves.push((i, list.clone(), index));
            }
        }
        for (i, list, index) in moves {
            self.switch(i, &list, index);
        }
    }

    /// Shows item `index` of `list` (or the next one that loads) on slot `i`.
    /// Starts converting a video for hardware decoding, at the size the
    /// monitors need.
    fn convert(&mut self, path: &Path) -> Result<()> {
        let video = session::wallpaper_path(path, Some(MediaKind::Video))
            .with_context(|| format!("no video in {}", path.display()))?;
        let probe = lumen_media::probe(&video)?;
        let outputs: Vec<(u32, u32)> = self
            .platform
            .outputs()?
            .iter()
            .map(|o| (o.width, o.height))
            .collect();
        let size = lumen_media::converted_size((probe.width, probe.height), &outputs);
        let waker = self.platform.waker();
        self.conversions.start(&video, size, move || waker.wake())
    }

    /// Loads `video` again in the slots showing it (to play its converted copy).
    fn reload_video(&mut self, video: &Path) {
        for i in 0..self.slots.len() {
            if !same_path(&self.slots[i].wallpaper.path, video) {
                continue;
            }
            let look = self.slots[i].wallpaper.look;
            match self.load(video, Some(MediaKind::Video), look) {
                Ok(w) => self.replace(i, w),
                Err(e) => tracing::warn!("reloading {}: {e:#}", video.display()),
            }
        }
    }

    /// Shows `wallpaper` in slot `i` instead of what it shows; the old one
    /// stays up until the new one has drawn.
    fn replace(&mut self, i: usize, wallpaper: Wallpaper) {
        let fps = self.fps;
        let slot = &mut self.slots[i];
        slot.wallpaper = wallpaper;
        if let Some(old) = slot.session.take() {
            retire(&mut self.retiring, old);
        }
        slot.shown_on.clear();
        slot.clock = VideoClock::default();
        slot.scene_clock = SceneClock::new(fps);
        self.rebuild(false);
        let volume = self.volume;
        self.slots[i].start_audio(volume);
    }

    fn switch(&mut self, i: usize, list: &Playlist, index: usize) {
        let layout = self.slots[i].wallpaper.look.layout;
        let n = list.items.len();
        for k in 0..n {
            let at = (index + k) % n;
            let path = &list.items[at];
            let look = Look {
                fit: self.default_fit(path),
                layout,
            };
            match self.load(path, None, look) {
                Ok(w) => {
                    tracing::info!(playlist = %list.name, path = %path.display(), "next wallpaper");
                    if let Some(run) = self.slots[i].run.as_mut() {
                        run.index = at;
                        run.next = Instant::now() + Run::wait(list);
                    }
                    self.replace(i, w);
                    self.persist();
                    return;
                }
                Err(e) => {
                    tracing::warn!(playlist = %list.name, "skipping {}: {e:#}", path.display())
                }
            }
        }
        // Nothing loads: keep what shows, try again later.
        if let Some(run) = self.slots[i].run.as_mut() {
            run.next = Instant::now() + Run::wait(list);
        }
    }

    /// Starts the playlist `name` on the monitors `claim` (empty: all).
    fn play(&mut self, name: &str, claim: Vec<String>) -> Result<()> {
        let list = self
            .playlists
            .iter()
            .find(|p| p.name == name)
            .cloned()
            .with_context(|| format!("no playlist {name}"))?;
        anyhow::ensure!(!list.items.is_empty(), "playlist {name} is empty");
        let first = playlist::first(&list, &mut self.rng);
        let layout = self
            .main_slot()
            .map_or(Layout::PerOutput, |s| s.wallpaper.look.layout);
        let n = list.items.len();
        let mut last_error = None;
        for k in 0..n {
            let at = (first + k) % n;
            let path = &list.items[at];
            let look = Look {
                fit: self.default_fit(path),
                layout,
            };
            match self.load(path, None, look) {
                Ok(w) => {
                    let shown = self.set_wallpaper(w, claim);
                    if let Some(slot) = self.slots.last_mut() {
                        slot.run = Some(Run::new(&list, at));
                    }
                    self.persist();
                    return shown;
                }
                Err(e) => {
                    tracing::warn!(playlist = %name, "skipping {}: {e:#}", path.display());
                    last_error = Some(e);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("playlist {name} is empty")))
    }

    /// Output ids for what the user typed (`None`: all monitors).
    fn claim(&self, outputs: Option<Vec<String>>) -> std::result::Result<Vec<String>, String> {
        match outputs {
            Some(names) => slots::resolve(&names, &self.platform.outputs().unwrap_or_default()),
            None => Ok(Vec::new()),
        }
    }

    /// Property values set in Lumen for the wallpaper at `path` (see
    /// [`session::wallpaper_path`]).
    fn overrides_of(&self, path: &Path) -> properties::Values {
        self.overrides
            .iter()
            .find(|(p, _)| same_path(p, path))
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    /// Loads a wallpaper with its property overrides.
    fn load(&self, path: &Path, kind: Option<MediaKind>, look: Look) -> Result<Wallpaper> {
        let overrides = session::wallpaper_path(path, kind)
            .map(|p| self.overrides_of(&p))
            .unwrap_or_default();
        Wallpaper::load(
            path,
            kind,
            look,
            self.we_assets.as_deref(),
            self.we_config.as_deref(),
            &overrides,
        )
    }

    /// Changes property values of a wallpaper; those showing it update.
    fn set_properties(
        &mut self,
        path: &Path,
        changes: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<()> {
        let key = session::wallpaper_path(path, None)
            .with_context(|| format!("unknown media type for {}", path.display()))?;
        let mut values = self.overrides_of(&key);
        properties::merge(&mut values, changes);
        self.overrides.retain(|(p, _)| !same_path(p, &key));
        if !values.is_empty() {
            self.overrides.push((key.clone(), values.clone()));
        }
        self.persist();
        // Pages hear the new values; scenes are imported again.
        let mut reload = Vec::new();
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if !same_path(&slot.wallpaper.path, &key) {
                continue;
            }
            match &mut slot.wallpaper.content {
                Content::Web(source) => {
                    source.overrides = values.clone();
                    let choices = source.choices();
                    if let Some(web) = slot.session.as_mut().and_then(Session::web) {
                        web.set_properties(choices);
                    }
                }
                Content::Scene(_) => reload.push(i),
                Content::Image(_) | Content::Video(_) => {}
            }
        }
        for &i in &reload {
            let (path, kind, look) = {
                let w = &self.slots[i].wallpaper;
                (w.path.clone(), w.kind, w.look)
            };
            let wallpaper = self.load(&path, Some(kind), look)?;
            let slot = &mut self.slots[i];
            slot.wallpaper = wallpaper;
            if let Some(old) = slot.session.take() {
                retire(&mut self.retiring, old);
            }
            slot.shown_on.clear();
        }
        if !reload.is_empty() {
            self.rebuild(false);
        }
        Ok(())
    }

    /// A wallpaper's property values as it sees them.
    fn properties(&self, path: &Path) -> Result<properties::Values> {
        let kind = MediaKind::from_path(path)
            .with_context(|| format!("unknown media type for {}", path.display()))?;
        let key = session::wallpaper_path(path, Some(kind))
            .with_context(|| format!("unknown media type for {}", path.display()))?;
        let dir = if key.is_dir() {
            key.clone()
        } else {
            key.parent().map(Path::to_path_buf).unwrap_or_default()
        };
        // WE keeps web pages' choices under the page itself.
        let we_key = match kind {
            MediaKind::Web => dir.join(
                properties::project(&dir)["file"]
                    .as_str()
                    .unwrap_or("index.html"),
            ),
            _ => key.clone(),
        };
        let mut values = properties::effective(
            &dir,
            &we_key,
            self.we_config.as_deref(),
            &self.overrides_of(&key),
        );
        // Scenes with camera parallax get its switch (WE's per-wallpaper
        // option, on unless someone turned it off).
        if kind == MediaKind::Scene
            && !values.contains_key("cameraparallax")
            && lumen_we::scene::uses_parallax(&key) == Some(true)
        {
            values.insert("cameraparallax".into(), serde_json::Value::Bool(true));
        }
        Ok(values)
    }

    fn network_allowed(&self, path: &Path) -> bool {
        self.web_allowed.iter().any(|p| same_path(p, path))
    }

    fn set_network_allowed(&mut self, path: &Path, allow: bool) {
        self.web_allowed.retain(|p| !same_path(p, path));
        if allow {
            self.web_allowed.push(path.to_path_buf());
        }
        tracing::info!(path = %path.display(), allow, "web wallpaper network access");
    }

    /// Remembers the wallpapers for the next start.
    fn persist(&self) {
        let Some(dir) = self.config_dir.as_deref() else {
            return;
        };
        let state = state::State {
            wallpapers: self.slots.iter().map(Slot::saved).collect(),
            volume: self.volume,
            web_network: self.web_allowed.clone(),
            playlists: self.playlists.clone(),
            folders: self.folders.clone(),
            policy: Some(self.policy),
            properties: self
                .overrides
                .iter()
                .map(|(path, values)| state::SavedProperties {
                    path: path.clone(),
                    values: values.clone(),
                })
                .collect(),
            ..state::State::default()
        };
        if let Err(e) = state::save(dir, &state) {
            tracing::warn!("could not save state: {e}");
        }
    }

    /// Recreates windows and surfaces: for every wallpaper (`all`), or only
    /// those whose monitors changed. Failures are retried next tick.
    fn rebuild(&mut self, all: bool) {
        self.needs_rebuild = false;
        if all {
            self.retiring.clear();
        }
        // Without a device, the GPU re-init in `tick` asks for a rebuild later.
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let infos = match self.platform.outputs() {
            Ok(infos) => infos,
            Err(e) => {
                tracing::warn!("enumerating outputs: {e:#}; retrying");
                self.needs_rebuild = true;
                std::thread::sleep(REBUILD_RETRY);
                return;
            }
        };
        let claims: Vec<Vec<String>> = self.slots.iter().map(|s| s.claim.clone()).collect();
        let groups = slots::assign(&claims, &infos);
        let networks: Vec<bool> = self
            .slots
            .iter()
            .map(|s| self.web_network || self.network_allowed(&s.wallpaper.path))
            .collect();
        let primary = infos.iter().find(|o| o.primary).map(|o| o.id.0.clone());
        for ((slot, group), network) in self.slots.iter_mut().zip(groups).zip(networks) {
            if all {
                slot.failures = 0;
            }
            let ids: Vec<String> = group.iter().map(|o| o.id.0.clone()).collect();
            let was_primary = slot.primary;
            slot.primary = primary.as_ref().is_some_and(|p| ids.contains(p));
            if slot.primary != was_primary {
                slot.start_audio(self.volume);
            }
            if !all && slot.session.is_some() && slot.shown_on == ids || slot.failures >= MAX_TRIES
            {
                continue;
            }
            // After a desktop reset or device loss the old surfaces and windows
            // go first (they reference the old device and Explorer windows);
            // otherwise the old wallpaper stays up until the new one has drawn.
            match slot.session.take() {
                Some(old) if !all => retire(&mut self.retiring, old),
                _ => {}
            }
            slot.shown_on = ids;
            if group.is_empty() {
                continue;
            }
            let below = !self.retiring.is_empty();
            match Session::build(self.platform, gpu, &slot.wallpaper, group, network, below) {
                Ok(mut session) => {
                    // Images are drawn while building; the rest after their first frame.
                    slot.shown = !below || slot.wallpaper.kind == MediaKind::Image;
                    if below && slot.shown {
                        session.raise();
                    }
                    if let Some(frame) = slot.clock.current.as_ref()
                        && let Err(e) = session.show_video_frame(gpu, frame)
                    {
                        tracing::warn!("re-showing current frame failed: {e:#}");
                    }
                    slot.session = Some(session);
                    slot.failures = 0;
                    slot.error = None;
                    slot.set_sound_volume(self.volume);
                }
                Err(e) => {
                    slot.failures += 1;
                    slot.error = Some(format!("{e:#}"));
                    slot.shown_on.clear();
                    if slot.failures < MAX_TRIES {
                        tracing::warn!(path = %slot.wallpaper.path.display(), "{e:#}; retrying");
                        self.needs_rebuild = true;
                    } else {
                        tracing::error!(
                            path = %slot.wallpaper.path.display(),
                            "cannot show this wallpaper: {e:#}"
                        );
                    }
                }
            }
        }
        self.next_coverage = Instant::now();
        if self.needs_rebuild {
            std::thread::sleep(REBUILD_RETRY);
        }
    }

    /// Shows `wallpaper` on the monitors `claim` (empty: all of them),
    /// replacing what they showed.
    fn set_wallpaper(&mut self, wallpaper: Wallpaper, claim: Vec<String>) -> Result<()> {
        let claims: Vec<Vec<String>> = self.slots.iter().map(|s| s.claim.clone()).collect();
        let left = slots::reclaim(&claims, &claim);
        let mut kept = Vec::new();
        for (mut slot, left) in self.slots.drain(..).zip(left) {
            if let Some(c) = left {
                slot.claim = c;
                kept.push(slot);
            } else if let Some(old) = slot.session.take() {
                retire(&mut self.retiring, old);
            }
        }
        self.slots = kept;
        let mut slot = Slot::new(wallpaper, claim, self.fps);
        if self.os_fallback {
            // Taken from the first frame, if it lands on the primary monitor.
            slot.fallback_pending = true;
        }
        self.slots.push(slot);
        self.persist();
        self.rebuild(false);
        let new = self.slots.last_mut().context("wallpaper just added")?;
        if self.os_fallback && new.primary && new.wallpaper.kind == MediaKind::Image {
            // Images are their own snapshot.
            new.fallback_pending = false;
            set_fallback(
                &new.wallpaper.path,
                new.wallpaper.look.layout == Layout::Span,
            );
        }
        if !new.primary {
            new.fallback_pending = false;
        }
        new.start_audio(self.volume);
        if let Some(error) = new.error.clone() {
            // Its monitors show nothing rather than keep a wallpaper that cannot show.
            self.slots.pop();
            self.persist();
            anyhow::bail!("{error}");
        }
        if self.needs_rebuild {
            anyhow::bail!(
                "wallpaper loaded but could not be shown yet; retrying in the background"
            );
        }
        Ok(())
    }

    /// The fit Wallpaper Engine uses for this wallpaper on the first monitor,
    /// so Lumen starts from the user's existing choice.
    fn we_fit(&self, path: &std::path::Path) -> Option<Fit> {
        we_fit(self.we_config.as_deref()?, path)
    }

    fn default_fit(&self, path: &std::path::Path) -> Fit {
        self.we_fit(path).unwrap_or(Fit::Fill)
    }

    /// The wallpaper on the primary monitor (else the first).
    /// The wallpaper playing alone on every monitor that shows anything
    /// (what its cost is measured on).
    fn showing_alone(&self) -> Option<costs::Showing> {
        if !self.retiring.is_empty() {
            return None;
        }
        let mut showing: Option<costs::Showing> = None;
        for slot in &self.slots {
            let Some(session) = &slot.session else {
                continue;
            };
            if !slot.shown || session.outputs.iter().any(|o| o.paused) {
                return None;
            }
            let pixels: u64 = session
                .outputs
                .iter()
                .map(|o| u64::from(o.info.width) * u64::from(o.info.height))
                .sum();
            match &mut showing {
                None => {
                    showing = Some(costs::Showing {
                        path: slot.wallpaper.path.clone(),
                        pixels,
                        // Only scenes run at the policy's frame rate.
                        fps: if slot.wallpaper.kind == MediaKind::Scene {
                            self.fps
                        } else {
                            0
                        },
                    });
                }
                Some(s) if s.path == slot.wallpaper.path => s.pixels += pixels,
                Some(_) => return None,
            }
        }
        showing
    }

    fn tray_state(&self) -> TrayState {
        TrayState {
            paused: self.conditions.user_paused,
            can_next: self.slots.iter().any(|s| s.run.is_some()),
        }
    }

    /// A choice from the notification-area icon: what the CLI would ask for.
    fn on_tray(&mut self, command: TrayCommand) {
        let request = match command {
            TrayCommand::OpenUi => return tray::open_ui(),
            TrayCommand::Next => Request::Next { outputs: None },
            TrayCommand::Pause => Request::Pause,
            TrayCommand::Resume => Request::Resume,
            TrayCommand::Quit => Request::Quit,
        };
        if let Response::Error { message } = self.handle(request) {
            tracing::warn!("tray: {message}");
        }
    }

    fn main_slot(&self) -> Option<&Slot> {
        self.slots.iter().find(|s| s.primary).or(self.slots.first())
    }

    fn handle(&mut self, request: Request) -> Response {
        tracing::info!(?request, "control request");
        let ok_or_error = |r: Result<()>| match r {
            Ok(()) => Response::Ok,
            Err(e) => Response::Error {
                message: format!("{e:#}"),
            },
        };
        match request {
            Request::Status => Response::Status(self.status()),
            Request::Set {
                path,
                kind,
                fit,
                layout,
                network,
                outputs,
            } => {
                let claim = match self.claim(outputs) {
                    Ok(ids) => ids,
                    Err(message) => return Response::Error { message },
                };
                let current = self.main_slot().map(|s| s.wallpaper.look);
                let look = Look {
                    fit: fit
                        .or_else(|| self.we_fit(&path))
                        .or(current.map(|l| l.fit))
                        .unwrap_or(Fit::Fill),
                    layout: layout
                        .or(current.map(|l| l.layout))
                        .unwrap_or(Layout::PerOutput),
                };
                ok_or_error(self.load(&path, kind, look).and_then(|w| {
                    if let Some(allow) = network {
                        self.set_network_allowed(&w.path, allow);
                    }
                    self.set_wallpaper(w, claim)
                }))
            }
            Request::SetProperties { path, properties } => {
                ok_or_error(self.set_properties(&path, &properties))
            }
            Request::Properties { path } => match self.properties(&path) {
                Ok(values) => Response::Properties {
                    values: values.into_iter().collect(),
                },
                Err(e) => Response::Error {
                    message: format!("{e:#}"),
                },
            },
            Request::Volume { percent } => {
                self.volume = percent.min(100);
                for slot in &mut self.slots {
                    match (&slot.audio, self.volume) {
                        (_, 0) => {
                            slot.audio = None;
                            slot.audio_loop = None;
                        }
                        (Some(audio), v) => audio.set_volume(f32::from(v) / 100.0),
                        (None, _) => slot.start_audio(self.volume),
                    }
                    slot.set_sound_volume(self.volume);
                }
                self.persist();
                Response::Ok
            }
            Request::SavePlaylist { playlist } => {
                if playlist.name.trim().is_empty() {
                    return Response::Error {
                        message: "a playlist needs a name".into(),
                    };
                }
                if let Schedule::TimeOfDay { starts } = &playlist.schedule
                    && starts.len() != playlist.items.len()
                {
                    return Response::Error {
                        message: "a time-of-day playlist needs one start time per wallpaper".into(),
                    };
                }
                self.playlists.retain(|p| p.name != playlist.name);
                self.playlists.push(playlist);
                self.persist();
                Response::Ok
            }
            Request::DeletePlaylist { name } => {
                self.playlists.retain(|p| p.name != name);
                for slot in &mut self.slots {
                    if slot.run.as_ref().is_some_and(|r| r.name == name) {
                        slot.run = None;
                    }
                }
                self.persist();
                Response::Ok
            }
            Request::Playlists => Response::Playlists {
                playlists: self.playlists.clone(),
            },
            Request::Costs => Response::Costs {
                costs: self.costs.list(),
            },
            Request::Convert { path } => ok_or_error(self.convert(&path)),
            Request::Play { name, outputs } => match self.claim(outputs) {
                Ok(claim) => ok_or_error(self.play(&name, claim)),
                Err(message) => Response::Error { message },
            },
            Request::Next { outputs } => match self.claim(outputs) {
                Ok(ids) => {
                    self.advance_playlists(Some(&ids));
                    Response::Ok
                }
                Err(message) => Response::Error { message },
            },
            Request::SetFolders { folders } => {
                self.folders = folders;
                self.persist();
                Response::Ok
            }
            Request::SetPolicy { policy } => {
                self.policy = Policy {
                    fps: policy.fps.clamp(1, 240),
                    ..policy
                };
                self.fps = self.policy.fps;
                for slot in &mut self.slots {
                    slot.scene_clock.set_fps(self.fps);
                }
                self.next_coverage = Instant::now();
                self.persist();
                Response::Ok
            }
            Request::Pause | Request::Resume => {
                self.conditions.user_paused = matches!(request, Request::Pause);
                self.next_coverage = Instant::now();
                Response::Ok
            }
            Request::Quit => {
                self.quit = true;
                Response::Ok
            }
        }
    }

    fn status(&self) -> Status {
        // Monitors with a wallpaper, then any showing nothing.
        let mut outputs: Vec<OutputStatus> = Vec::new();
        for slot in &self.slots {
            let Some(session) = &slot.session else {
                continue;
            };
            for o in &session.outputs {
                let mut status = output_status(
                    o.info.clone(),
                    o.paused,
                    o.coverage,
                    Some(slot.content_status()),
                );
                status.playlist = slot.run.as_ref().map(|r| r.name.clone());
                outputs.push(status);
            }
        }
        for info in self.platform.outputs().unwrap_or_default() {
            if !outputs.iter().any(|o| o.id == info.id.0) {
                outputs.push(output_status(info, false, Coverage::Visible, None));
            }
        }
        outputs.sort_by_key(|o| (o.x, o.y));
        Status {
            pid: std::process::id(),
            daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
            content: self.main_slot().map(Slot::content_status),
            user_paused: self.conditions.user_paused,
            volume: self.volume,
            policy: self.policy,
            folders: self.folders.clone(),
            outputs,
            conversions: self.conversions.status(),
        }
    }
}

fn output_status(
    info: lumen_platform::OutputInfo,
    paused: bool,
    coverage: Coverage,
    content: Option<ContentStatus>,
) -> OutputStatus {
    OutputStatus {
        id: info.id.0,
        device: info.device,
        name: info.name,
        x: info.x,
        y: info.y,
        width: info.width,
        height: info.height,
        refresh_hz: f64::from(info.refresh_mhz) / 1000.0,
        primary: info.primary,
        paused,
        coverage: format!("{coverage:?}").to_lowercase(),
        content,
        playlist: None,
    }
}
