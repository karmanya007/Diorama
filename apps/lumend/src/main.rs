//! `lumend` — Lumen wallpaper daemon.
//!
//! Shows an image or a looping video behind the desktop icons on every
//! monitor, controlled by the `lumen` CLI over a per-user local socket.
//! Static images render once (≈0% GPU idle, P2); video frames are presented on
//! their timestamps. Outputs pause individually when covered (P5/P6). Windows
//! and surfaces are rebuilt when Explorer restarts (P9), monitors change (P8)
//! or the GPU is lost (P11). Video never opens an audio device (P10).

mod clock;
mod headless;
mod policy;
mod session;
mod state;

use std::path::PathBuf;
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
use lumen_platform::{Coverage, DesktopEvent, Platform};

use crate::clock::{SceneClock, VideoClock};
use crate::policy::{Action, Conditions, Policy, update_pauses};
use crate::session::{Content, Look, Session, Wallpaper};

#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Image to show at start-up (PNG, JPEG, …). Optional: `lumen set` can choose later.
    #[arg(long, conflicts_with_all = ["video", "scene"])]
    image: Option<PathBuf>,
    /// Video to loop at start-up (MP4, …). Muted.
    #[arg(long, conflicts_with = "scene")]
    video: Option<PathBuf>,
    /// Wallpaper Engine scene project folder (contains `scene.pkg` or `scene.json`).
    #[arg(long)]
    scene: Option<PathBuf>,
    /// Wallpaper Engine `assets` folder (built-in shaders and effects used by scenes).
    #[arg(long, default_value = headless::DEFAULT_WE_ASSETS)]
    we_assets: PathBuf,
    /// Render one frame of `--scene` offscreen to this PNG and exit.
    #[arg(long, requires = "scene")]
    frame_out: Option<PathBuf>,
    /// Frame rate cap for scenes.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u32).range(1..=240))]
    fps: u32,
    /// Scene time in seconds for `--frame-out`.
    #[arg(long, default_value_t = 0.0)]
    time: f32,
    /// With `--frame-out`: also save every intermediate render target beside it.
    #[arg(long, requires = "frame_out")]
    dump_targets: bool,
    #[arg(long, value_enum, default_value = "fill")]
    fit: FitArg,
    /// `per-output`: the whole picture on every monitor. `span`: one picture across all monitors.
    #[arg(long, value_enum, default_value = "per-output")]
    layout: LayoutArg,
    #[arg(long, value_enum, default_value = "auto")]
    backend: BackendArg,
    /// Prefer an integrated GPU.
    #[arg(long)]
    low_power: bool,
    /// Exit after this many seconds (for testing).
    #[arg(long)]
    duration: Option<u64>,
    /// Print detected outputs and exit.
    #[arg(long)]
    list_outputs: bool,
    /// When a fullscreen app covers a monitor (games, video players).
    #[arg(long, value_enum, default_value = "pause")]
    on_fullscreen: Action,
    /// When a maximized window covers a monitor.
    #[arg(long, value_enum, default_value = "pause")]
    on_maximized: Action,
    /// While running on battery.
    #[arg(long, value_enum, default_value = "keep")]
    on_battery: Action,
    /// While the session is locked.
    #[arg(long, value_enum, default_value = "pause")]
    on_lock: Action,
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
const AUDIO_CHECK_INTERVAL: Duration = Duration::from_secs(2);

type Job = (Request, SyncSender<Response>);

fn main() -> Result<()> {
    let args = Args::parse();
    let config_dir = lumen_platform::config_dir();
    init_logging(args.background, config_dir.as_deref());
    let platform = Platform::new().context("platform init")?;

    if let (Some(scene), Some(out)) = (&args.scene, &args.frame_out) {
        let assets = args.we_assets.is_dir().then_some(args.we_assets.as_path());
        return headless::render_to_png(
            scene,
            assets,
            gpu_options(&args),
            args.time,
            out,
            args.dump_targets,
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
        gpu: Gpu::new(gpu_options).context("GPU init")?,
        gpu_options,
        policy: Policy {
            fullscreen: args.on_fullscreen,
            maximized: args.on_maximized,
            battery: args.on_battery,
            locked: args.on_lock,
        },
        conditions: Conditions {
            on_battery: platform.on_battery(),
            ..Conditions::default()
        },
        wallpaper: None,
        session: None,
        config_dir: config_dir.clone(),
        os_fallback: args.os_fallback,
        fallback_pending: false,
        volume: 0,
        audio: None,
        audio_loop: None,
        next_audio_check: Instant::now() + AUDIO_CHECK_INTERVAL,
        clock: VideoClock::default(),
        fps: args.fps,
        scene_clock: SceneClock::new(args.fps),
        we_assets: args.we_assets.is_dir().then(|| args.we_assets.clone()),
        next_coverage: Instant::now(),
        next_battery: Instant::now() + BATTERY_INTERVAL,
        needs_rebuild: false,
        quit: false,
    };

    let look = Look {
        fit: match args.fit {
            FitArg::Fill => Fit::Fill,
            FitArg::Fit => Fit::Fit,
            FitArg::Stretch => Fit::Stretch,
        },
        layout: match args.layout {
            LayoutArg::PerOutput => Layout::PerOutput,
            LayoutArg::Span => Layout::Span,
        },
    };
    // Start-up content: command line, else the wallpaper from last time.
    let saved_state = config_dir.as_deref().map(state::load).unwrap_or_default();
    daemon.volume = args.volume.unwrap_or(saved_state.volume).min(100);
    let saved = saved_state.wallpaper;
    let initial = match (
        args.image
            .as_ref()
            .or(args.video.as_ref())
            .or(args.scene.as_ref()),
        saved,
    ) {
        (Some(path), _) => Some((path.clone(), None, look)),
        (None, Some(s)) => Some((
            s.path,
            Some(s.kind),
            Look {
                fit: s.fit,
                layout: s.layout,
            },
        )),
        (None, None) => None,
    };
    match initial {
        Some((path, kind, look)) => {
            match Wallpaper::load(&path, kind, look, daemon.we_assets.as_deref()) {
                Ok(w) => {
                    if let Err(e) = daemon.set_wallpaper(w) {
                        tracing::warn!("{e:#}");
                    }
                }
                Err(e) => tracing::error!("start-up wallpaper: {e:#}"),
            }
        }
        None => tracing::info!("no wallpaper yet; waiting for `lumen set <file>`"),
    }

    while !stop.load(Ordering::SeqCst)
        && !daemon.quit
        && deadline.is_none_or(|d| Instant::now() < d)
    {
        daemon.tick(&jobs)?;
    }
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

struct Daemon<'p> {
    platform: &'p Platform,
    gpu: Gpu,
    gpu_options: GpuOptions,
    policy: Policy,
    conditions: Conditions,
    wallpaper: Option<Wallpaper>,
    session: Option<Session>,
    clock: VideoClock,
    fps: u32,
    scene_clock: SceneClock,
    /// `<WE install>/assets` for scene built-ins, if present.
    we_assets: Option<PathBuf>,
    next_coverage: Instant,
    next_battery: Instant,
    /// A rebuild failed (e.g. Explorer still starting); retry on the next tick.
    needs_rebuild: bool,
    quit: bool,
    config_dir: Option<PathBuf>,
    os_fallback: bool,
    /// Video fallback snapshot still to be taken from the next shown frame.
    fallback_pending: bool,
    /// Soundtrack volume 0–100; 0 means no audio player at all.
    volume: u8,
    audio: Option<AudioPlayer>,
    /// Video loop index the audio was last synced to; `None` = not started yet.
    audio_loop: Option<u32>,
    next_audio_check: Instant,
}

impl Daemon<'_> {
    /// One iteration: wait for events/IPC/next frame, then react.
    fn tick(&mut self, jobs: &Receiver<Job>) -> Result<()> {
        let content_wait = match self.wallpaper.as_ref().map(|w| &w.content) {
            Some(Content::Video(_)) if self.clock.is_paused() => COVERAGE_INTERVAL,
            Some(Content::Video(player)) => self.clock.time_until_next(player),
            Some(Content::Scene(_)) if self.scene_clock.is_paused() => COVERAGE_INTERVAL,
            Some(Content::Scene(_)) => self.scene_clock.time_until_next(),
            _ => Duration::from_secs(1),
        };
        let wait = content_wait.min(self.next_coverage.saturating_duration_since(Instant::now()));
        let events = self.platform.poll_events(wait);
        self.gpu.maintain();

        while let Ok((request, reply)) = jobs.try_recv() {
            let response = self.handle(request);
            let _ = reply.send(response);
        }

        for e in &events {
            match e {
                DesktopEvent::SessionLocked => self.conditions.locked = true,
                DesktopEvent::SessionUnlocked => self.conditions.locked = false,
                _ => {}
            }
        }
        if Instant::now() >= self.next_battery {
            self.conditions.on_battery = self.platform.on_battery();
            self.next_battery = Instant::now() + BATTERY_INTERVAL;
        }

        let device_lost = self.gpu.is_lost();
        if device_lost {
            self.session = None;
            self.gpu = Gpu::new(self.gpu_options).context("GPU re-init after device loss")?;
        }
        if events.iter().any(|e| e.needs_rebuild()) || device_lost || self.needs_rebuild {
            tracing::info!(?events, device_lost, "rebuilding wallpaper");
            self.rebuild();
            return Ok(());
        }

        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        if Instant::now() >= self.next_coverage || events.iter().any(|e| !e.needs_rebuild()) {
            self.next_coverage = Instant::now() + COVERAGE_INTERVAL;
            let resumed = update_pauses(self.platform, session, self.policy, self.conditions);
            if let Some(Content::Video(_)) = self.wallpaper.as_ref().map(|w| &w.content) {
                let all_paused = session.outputs.iter().all(|o| o.paused);
                self.clock.set_paused(all_paused);
                if let (Some(audio), Some(_)) = (&self.audio, self.audio_loop) {
                    audio.set_paused(all_paused);
                }
                // Freshly resumed outputs still show an old frame: present the current one now.
                if resumed {
                    session.present_all(&self.gpu)?;
                }
            }
            if let Some(Content::Scene(_)) = self.wallpaper.as_ref().map(|w| &w.content) {
                // Resuming makes the next frame due immediately.
                self.scene_clock
                    .set_paused(session.outputs.iter().all(|o| o.paused));
            }
        }

        if let Some(Content::Scene(_)) = self.wallpaper.as_ref().map(|w| &w.content)
            && self.scene_clock.frame_due()
        {
            session.show_scene_frame(&self.gpu, self.scene_clock.time())?;
            if self.fallback_pending {
                self.fallback_pending = false;
                let span = self
                    .wallpaper
                    .as_ref()
                    .is_some_and(|w| w.look.layout == Layout::Span);
                scene_fallback(self.config_dir.as_deref(), &self.gpu, session, span);
            }
        }

        if let Some(Content::Video(player)) = self.wallpaper.as_ref().map(|w| &w.content)
            && !self.clock.is_paused()
            && let Some(frame) = self.clock.due_frame(player)
        {
            let loop_index = frame.loop_index;
            session.show_video_frame(&self.gpu, frame)?;
            if self.fallback_pending {
                self.fallback_pending = false;
                self.video_fallback();
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

        if Instant::now() >= self.next_audio_check {
            self.next_audio_check = Instant::now() + AUDIO_CHECK_INTERVAL;
            if self.audio.as_ref().is_some_and(AudioPlayer::needs_reopen) {
                tracing::info!("audio device changed; reopening");
                self.start_audio();
            }
        }
        Ok(())
    }

    /// (Re)creates the soundtrack player for the current video, if volume > 0.
    /// Errors only mean silence; video is unaffected (P10).
    fn start_audio(&mut self) {
        self.audio = None;
        self.audio_loop = None;
        let Some(w) = self.wallpaper.as_ref() else {
            return;
        };
        if self.volume == 0 || w.kind != MediaKind::Video {
            return;
        }
        match AudioPlayer::spawn(&w.path, f32::from(self.volume) / 100.0) {
            Ok(a) => self.audio = Some(a),
            Err(e) => tracing::info!("no video audio: {e}"),
        }
    }

    /// Remembers the wallpaper for the next start.
    fn persist(&self, w: &Wallpaper) {
        let Some(dir) = self.config_dir.as_deref() else {
            return;
        };
        let state = state::State {
            wallpaper: Some(state::SavedWallpaper {
                path: w.path.clone(),
                kind: w.kind,
                fit: w.look.fit,
                layout: w.look.layout,
            }),
            volume: self.volume,
            ..state::State::default()
        };
        if let Err(e) = state::save(dir, &state) {
            tracing::warn!("could not save state: {e}");
        }
    }

    /// Saves the current video frame as a PNG and makes it the OS wallpaper.
    fn video_fallback(&self) {
        let (Some(dir), Some(frame), Some(w)) = (
            self.config_dir.as_deref(),
            self.clock.current.as_ref(),
            self.wallpaper.as_ref(),
        ) else {
            return;
        };
        let path = dir.join("fallback.png");
        let saved = image::RgbaImage::from_raw(frame.width, frame.height, frame.to_rgba())
            .map(|img| img.save(&path));
        match saved {
            Some(Ok(())) => set_fallback(&path, w.look.layout == Layout::Span),
            Some(Err(e)) => tracing::warn!("fallback snapshot not saved: {e}"),
            None => tracing::warn!("fallback snapshot has wrong size"),
        }
    }

    /// Recreates windows/surfaces for the current wallpaper. Failures are retried next tick.
    fn rebuild(&mut self) {
        // Surfaces/windows go first; they reference the old device and old Explorer windows.
        self.session = None;
        self.needs_rebuild = false;
        let Some(wallpaper) = self.wallpaper.as_ref() else {
            return;
        };
        match Session::build(self.platform, &self.gpu, wallpaper) {
            Ok(mut session) => {
                if let Some(frame) = self.clock.current.as_ref()
                    && let Err(e) = session.show_video_frame(&self.gpu, frame)
                {
                    tracing::warn!("re-showing current frame failed: {e:#}");
                }
                self.session = Some(session);
                self.next_coverage = Instant::now();
            }
            Err(e) => {
                tracing::warn!("{e:#}; retrying");
                self.needs_rebuild = true;
                std::thread::sleep(REBUILD_RETRY);
            }
        }
    }

    fn set_wallpaper(&mut self, wallpaper: Wallpaper) -> Result<()> {
        self.session = None;
        self.audio = None;
        self.clock = VideoClock::default();
        self.scene_clock = SceneClock::new(self.fps);
        self.persist(&wallpaper);
        if self.os_fallback {
            match wallpaper.kind {
                // Images are their own snapshot.
                MediaKind::Image => {
                    set_fallback(&wallpaper.path, wallpaper.look.layout == Layout::Span);
                }
                MediaKind::Video | MediaKind::Scene => self.fallback_pending = true,
            }
        }
        self.wallpaper = Some(wallpaper);
        self.start_audio();
        self.rebuild();
        if self.needs_rebuild {
            anyhow::bail!(
                "wallpaper loaded but could not be shown yet; retrying in the background"
            );
        }
        Ok(())
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
            } => {
                let current = self.wallpaper.as_ref().map(|w| w.look);
                let look = Look {
                    fit: fit.or(current.map(|l| l.fit)).unwrap_or(Fit::Fill),
                    layout: layout
                        .or(current.map(|l| l.layout))
                        .unwrap_or(Layout::PerOutput),
                };
                ok_or_error(
                    Wallpaper::load(&path, kind, look, self.we_assets.as_deref())
                        .and_then(|w| self.set_wallpaper(w)),
                )
            }
            Request::Volume { percent } => {
                self.volume = percent.min(100);
                match (&self.audio, self.volume) {
                    (_, 0) => {
                        self.audio = None;
                        self.audio_loop = None;
                    }
                    (Some(audio), v) => audio.set_volume(f32::from(v) / 100.0),
                    (None, _) => self.start_audio(),
                }
                if let Some(w) = self.wallpaper.as_ref() {
                    self.persist(w);
                }
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
        let outputs = match &self.session {
            Some(s) => s
                .outputs
                .iter()
                .map(|o| (o.info.clone(), o.paused, o.coverage))
                .collect(),
            None => self
                .platform
                .outputs()
                .unwrap_or_default()
                .into_iter()
                .map(|info| (info, false, Coverage::Visible))
                .collect::<Vec<_>>(),
        };
        Status {
            pid: std::process::id(),
            daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
            content: self.wallpaper.as_ref().map(|w| ContentStatus {
                kind: w.kind,
                path: w.path.clone(),
                fit: w.look.fit,
                layout: w.look.layout,
            }),
            user_paused: self.conditions.user_paused,
            volume: self.volume,
            outputs: outputs
                .into_iter()
                .map(|(info, paused, coverage)| OutputStatus {
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
                })
                .collect(),
        }
    }
}
