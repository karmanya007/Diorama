//! `lumend` — Lumen wallpaper daemon.
//!
//! Shows an image or a looping video behind the desktop icons on every
//! monitor, controlled by the `lumen` CLI over a per-user local socket.
//! Static images render once (≈0% GPU idle, P2); video frames are presented on
//! their timestamps. Outputs pause individually when covered (P5/P6). Windows
//! and surfaces are rebuilt when Explorer restarts (P9), monitors change (P8)
//! or the GPU is lost (P11). Video never opens an audio device (P10).

mod clock;
mod policy;
mod session;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use lumen_gpu::{Backend, Gpu, GpuOptions};
use lumen_ipc::{ContentStatus, Fit, Layout, OutputStatus, Request, Response, Status};
use lumen_platform::{Coverage, DesktopEvent, Platform};

use crate::clock::VideoClock;
use crate::policy::{Action, Conditions, Policy, update_pauses};
use crate::session::{Content, Look, Session, Wallpaper};

#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Image to show at start-up (PNG, JPEG, …). Optional: `lumen set` can choose later.
    #[arg(long, conflicts_with = "video")]
    image: Option<PathBuf>,
    /// Video to loop at start-up (MP4, …). Muted.
    #[arg(long)]
    video: Option<PathBuf>,
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

const COVERAGE_INTERVAL: Duration = Duration::from_millis(500);
const BATTERY_INTERVAL: Duration = Duration::from_secs(5);
const REBUILD_RETRY: Duration = Duration::from_secs(1);

type Job = (Request, SyncSender<Response>);

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let args = Args::parse();
    let platform = Platform::new().context("platform init")?;

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

    let gpu_options = GpuOptions {
        backend: match args.backend {
            BackendArg::Auto => Backend::Auto,
            BackendArg::Dx12 => Backend::Dx12,
            BackendArg::Vulkan => Backend::Vulkan,
        },
        low_power: args.low_power,
    };
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
        clock: VideoClock::default(),
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
    if let Some(path) = args.image.as_ref().or(args.video.as_ref()) {
        let wallpaper = Wallpaper::load(path, None, look)?;
        daemon.set_wallpaper(wallpaper)?;
    } else {
        tracing::info!("no wallpaper yet; waiting for `lumen set <file>`");
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

struct Daemon<'p> {
    platform: &'p Platform,
    gpu: Gpu,
    gpu_options: GpuOptions,
    policy: Policy,
    conditions: Conditions,
    wallpaper: Option<Wallpaper>,
    session: Option<Session>,
    clock: VideoClock,
    next_coverage: Instant,
    next_battery: Instant,
    /// A rebuild failed (e.g. Explorer still starting); retry on the next tick.
    needs_rebuild: bool,
    quit: bool,
}

impl Daemon<'_> {
    /// One iteration: wait for events/IPC/next frame, then react.
    fn tick(&mut self, jobs: &Receiver<Job>) -> Result<()> {
        let content_wait = match self.wallpaper.as_ref().map(|w| &w.content) {
            Some(Content::Video(_)) if self.clock.is_paused() => COVERAGE_INTERVAL,
            Some(Content::Video(player)) => self.clock.time_until_next(player),
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
                self.clock
                    .set_paused(session.outputs.iter().all(|o| o.paused));
                // Freshly resumed outputs still show an old frame: present the current one now.
                if resumed {
                    session.present_all(&self.gpu)?;
                }
            }
        }

        if let Some(Content::Video(player)) = self.wallpaper.as_ref().map(|w| &w.content)
            && !self.clock.is_paused()
            && let Some(frame) = self.clock.due_frame(player)
        {
            session.show_video_frame(&self.gpu, frame)?;
        }
        Ok(())
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
        self.clock = VideoClock::default();
        self.wallpaper = Some(wallpaper);
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
                ok_or_error(Wallpaper::load(&path, kind, look).and_then(|w| self.set_wallpaper(w)))
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
