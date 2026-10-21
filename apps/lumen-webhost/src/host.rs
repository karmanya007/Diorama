//! The web wallpaper host proper (needs a WebView: WebView2 on Windows).

use crate::{properties, protocol};

use std::collections::HashMap;
use std::io::{BufRead, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use lumen_platform::{OutputInfo, Platform, WallpaperWindow};
use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
use serde_json::{Map, Value};
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::{Rect, WebContext, WebView, WebViewBuilder};

const SHIM: &str = include_str!("shim.js");

#[derive(Parser, Debug)]
#[command(about = "Hosts a Wallpaper Engine web wallpaper behind the desktop icons")]
struct Args {
    /// The wallpaper folder (with project.json).
    dir: PathBuf,
    /// Page to open, relative to the folder (default: project.json `file`).
    #[arg(long)]
    file: Option<String>,
    /// Let the page reach the network (off: every request but the
    /// wallpaper's own files fails).
    #[arg(long)]
    allow_network: bool,
    /// Folder for WebView2 profiles (localStorage, caches), one per
    /// wallpaper and network mode. Default: `%LOCALAPPDATA%\Lumen\webview`.
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Allow the developer tools (F12 / context menu).
    #[arg(long)]
    devtools: bool,
    /// One page across all monitors (their bounding box) instead of one per monitor.
    #[arg(long)]
    span: bool,
    /// Instead of showing the wallpaper, render it off-screen and save a
    /// PNG of the page after `--after` seconds (tests; nothing on the desktop).
    #[arg(long)]
    snapshot: Option<PathBuf>,
    /// Page size for `--snapshot`, e.g. 1920x1080.
    #[arg(long, default_value = "1920x1080")]
    size: String,
    /// Seconds to let the page run before `--snapshot`.
    #[arg(long, default_value_t = 5.0)]
    after: f64,
}

#[derive(Debug, Clone, PartialEq)]
enum Pause {
    All(bool),
    Outputs(HashMap<String, bool>),
}

/// One message from lumend.
enum Command {
    /// `{name: value}` user choices over the project's defaults.
    Properties(Map<String, Value>),
    /// `{fps}`.
    General(Value),
    /// Every page (`true`/`false`) or per output id (`{id: bool}`).
    Paused(Pause),
    /// 128 levels: 64 left, 64 right.
    Audio(Vec<f32>),
    /// (kind, event): status | properties | thumbnail | playback | timeline.
    Media(String, Value),
    /// Desktop pixel coordinates and move | down | up.
    Pointer(i32, i32, String),
    /// Save a PNG of the (primary) page here: lumend's OS fallback wallpaper.
    Snapshot(PathBuf),
}

fn parse(line: &str) -> Option<Command> {
    let v: Value = serde_json::from_str(line).ok()?;
    let (key, value) = v.as_object()?.iter().next()?;
    Some(match key.as_str() {
        "properties" => Command::Properties(value.as_object()?.clone()),
        "general" => Command::General(value.clone()),
        "paused" => Command::Paused(match value {
            Value::Bool(b) => Pause::All(*b),
            Value::Object(m) => Pause::Outputs(
                m.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_bool()?)))
                    .collect(),
            ),
            _ => return None,
        }),
        "audio" => Command::Audio(
            value
                .as_array()?
                .iter()
                .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                .collect(),
        ),
        "media" => Command::Media(value["kind"].as_str()?.to_owned(), value["event"].clone()),
        "pointer" => Command::Pointer(
            i32::try_from(value["x"].as_i64()?).ok()?,
            i32::try_from(value["y"].as_i64()?).ok()?,
            value["kind"].as_str()?.to_owned(),
        ),
        "snapshot" => Command::Snapshot(PathBuf::from(value.as_str()?)),
        _ => return None,
    })
}

/// A raw handle as `HasWindowHandle` for wry.
struct Parent(RawWindowHandle);

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: the handle comes from a WallpaperWindow that outlives the
        // WebView built on it (both live in the same `Screen`).
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

/// A WebView on one output. Field order: the WebView goes before its window.
struct Screen {
    webview: WebView,
    window: WallpaperWindow,
    output: OutputInfo,
}

struct Host {
    root: PathBuf,
    url: String,
    allow_network: bool,
    devtools: bool,
    context: WebContext,
    /// The init script: the shim plus the current properties.
    properties: Value,
    general: Value,
    /// Pause per output id; outputs not listed use `paused_all`.
    paused: HashMap<String, bool>,
    paused_all: bool,
}

impl Host {
    fn is_paused(&self, output: &OutputInfo) -> bool {
        self.paused
            .get(&output.id.0)
            .copied()
            .unwrap_or(self.paused_all)
    }

    fn init_script(&self, output: &OutputInfo) -> String {
        format!(
            "{SHIM}\n__lumen.properties({});\n__lumen.general({});\n__lumen.paused({});",
            self.properties,
            self.general,
            self.is_paused(output)
        )
    }

    fn screen(&mut self, platform: &Platform, output: &OutputInfo) -> Result<Screen> {
        let window = platform
            .create_wallpaper(output)
            .context("creating the wallpaper window")?;
        self.page(window, output)
    }

    fn page(&mut self, window: WallpaperWindow, output: &OutputInfo) -> Result<Screen> {
        let (w, h) = window.size();
        let parent = Parent(window.handles().window);
        let root = self.root.clone();
        let origin = self.url.split('/').take(3).collect::<Vec<_>>().join("/");
        let allowed = [origin.clone(), wry_origin(&origin)];
        let init = self.init_script(output);
        let (allow_network, devtools) = (self.allow_network, self.devtools);
        let builder = WebViewBuilder::new_with_web_context(&mut self.context)
            .with_url(&self.url)
            .with_bounds(Rect {
                position: PhysicalPosition::new(0, 0).into(),
                size: PhysicalSize::new(w, h).into(),
            })
            .with_initialization_script(init)
            .with_custom_protocol("lumen".into(), move |_, request| {
                protocol::serve(&root, &request)
            })
            // Pages stay on the wallpaper; links and popups go nowhere.
            .with_navigation_handler(move |url| {
                allowed.iter().any(|o| url.starts_with(o.as_str())) || url == "about:blank"
            })
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_autoplay(true)
            .with_devtools(devtools);
        let builder = platform_options(builder, allow_network, devtools);
        let webview = builder
            .build_as_child(&parent)
            .context("creating the WebView (is the WebView2 runtime installed?)")?;
        Ok(Screen {
            webview,
            window,
            output: output.clone(),
        })
    }
}

/// `lumen://host` → the `https://lumen.host` form WebView2 navigates to.
fn wry_origin(origin: &str) -> String {
    origin.replacen("lumen://", "https://lumen.", 1)
}

#[cfg(windows)]
fn platform_options(
    builder: WebViewBuilder<'_>,
    allow_network: bool,
    devtools: bool,
) -> WebViewBuilder<'_> {
    use wry::WebViewBuilderExtWindows;
    // Behind the icons (or off-screen for snapshots) Chromium would think the
    // page is occluded and stop drawing it; lumend decides when to pause.
    let mut args = String::from(
        "--autoplay-policy=no-user-gesture-required          --disable-features=CalculateNativeWinOcclusion          --disable-backgrounding-occluded-windows --disable-renderer-backgrounding",
    );
    if !allow_network {
        // Every request goes through a proxy that is not there, loopback
        // included; the wallpaper's files never reach the network stack.
        args.push_str(
            " --proxy-server=http://127.0.0.1:9 --proxy-bypass-list=<-loopback> \
             --force-webrtc-ip-handling-policy=disable_non_proxied_udp",
        );
    }
    builder
        .with_https_scheme(true)
        .with_additional_browser_args(args)
        .with_browser_accelerator_keys(false)
        .with_default_context_menus(devtools)
}

#[cfg(not(windows))]
fn platform_options(
    builder: WebViewBuilder<'_>,
    _allow_network: bool,
    _devtools: bool,
) -> WebViewBuilder<'_> {
    builder
}

fn default_data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Lumen")
        .join("webview")
}

/// A host name unique to the wallpaper folder, so pages keep separate
/// `localStorage`.
fn host_name(dir: &Path) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in dir.to_string_lossy().to_lowercase().bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("w{h:016x}")
}

fn run(args: Args) -> Result<()> {
    let root = args
        .dir
        .canonicalize()
        .with_context(|| format!("wallpaper folder {}", args.dir.display()))?;
    let project = properties::read_project(&root);
    let file = args
        .file
        .clone()
        .or_else(|| project["file"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "index.html".into());
    anyhow::ensure!(
        protocol::resolve(&root, &file).is_some(),
        "{file} is not a file inside {}",
        root.display()
    );

    // Commands: lumend's stdin, read on a thread; the first may be the
    // initial properties, so read it before building the pages.
    let platform = Platform::new()?;
    let (tx, rx) = mpsc::channel::<Option<Command>>();
    let driven = !std::io::stdin().is_terminal();
    if driven {
        let waker = platform.waker();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if let Some(c) = parse(&line)
                    && tx.send(Some(c)).is_err()
                {
                    return;
                }
                waker.wake();
            }
            let _ = tx.send(None);
            waker.wake();
        });
    }
    let mut choices = Map::new();
    let mut early = Vec::new();
    if driven {
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Some(Command::Properties(c))) => choices = c,
            Ok(Some(other)) => early.push(other),
            Ok(None) if args.snapshot.is_none() => return Ok(()),
            Ok(None) | Err(_) => {}
        }
    }

    // One profile per wallpaper and network mode: WebView2 runs one browser
    // per profile and a browser still running from an earlier launch keeps
    // that launch's arguments, so a shared profile could hand a network-off
    // wallpaper a browser started with network on.
    let host_id = host_name(&root);
    let data_dir = args
        .data_dir
        .clone()
        .unwrap_or_else(default_data_dir)
        .join(format!(
            "{host_id}{}",
            if args.allow_network { "-net" } else { "" }
        ));
    let _ = std::fs::create_dir_all(&data_dir);
    let mut host = Host {
        url: format!("lumen://{host_id}/{}", file.trim_start_matches('/')),
        root: root.clone(),
        allow_network: args.allow_network,
        devtools: args.devtools,
        context: WebContext::new(Some(data_dir)),
        properties: properties::user_properties(&project, &choices),
        general: serde_json::json!({ "fps": 30 }),
        paused: HashMap::new(),
        paused_all: false,
    };
    tracing::info!(url = %host.url, network = host.allow_network, "web wallpaper");

    if let Some(out) = &args.snapshot {
        return snapshot(&mut host, &platform, &args, out, early, &rx);
    }
    let mut screens = build(&mut host, &platform, args.span)?;
    for c in early {
        apply(&mut host, &screens, &platform, c);
    }
    loop {
        for event in platform.poll_events(Duration::from_millis(500)) {
            if event.needs_rebuild() {
                tracing::info!(?event, "rebuilding web wallpaper windows");
                screens.clear();
                screens = build(&mut host, &platform, args.span)?;
            }
        }
        for s in &screens {
            s.window.keep_in_place();
        }
        while let Ok(c) = rx.try_recv() {
            match c {
                Some(c) => apply(&mut host, &screens, &platform, c),
                None => return Ok(()),
            }
        }
    }
}

/// Writes the primary page's capture to `path` (via a temporary file, so a
/// reader never sees half a PNG).
fn save_snapshot(screens: &[Screen], platform: &Platform, path: &Path) {
    let Some(screen) = screens
        .iter()
        .find(|s| s.output.primary)
        .or_else(|| screens.first())
    else {
        return;
    };
    let tmp = path.with_extension("tmp");
    let result = capture(&screen.webview, platform)
        .and_then(|png| std::fs::write(&tmp, png).context("writing the snapshot"))
        .and_then(|()| std::fs::rename(&tmp, path).context("moving the snapshot"));
    match result {
        Ok(()) => tracing::info!(path = %path.display(), "web wallpaper snapshot"),
        Err(e) => tracing::warn!("web wallpaper snapshot: {e:#}"),
    }
}

/// The bounding box of `outputs` as one output (span layout).
fn span_output(outputs: &[OutputInfo]) -> Option<OutputInfo> {
    let x0 = outputs.iter().map(|o| o.x).min()?;
    let y0 = outputs.iter().map(|o| o.y).min()?;
    let x1 = outputs.iter().map(|o| o.x + o.width as i32).max()?;
    let y1 = outputs.iter().map(|o| o.y + o.height as i32).max()?;
    Some(OutputInfo {
        id: lumen_platform::OutputId("span".into()),
        name: String::new(),
        manufacturer: String::new(),
        product_code: 0,
        device: "span".into(),
        x: x0,
        y: y0,
        width: u32::try_from(x1 - x0).ok()?,
        height: u32::try_from(y1 - y0).ok()?,
        refresh_mhz: 0,
        primary: true,
    })
}

/// `--snapshot`: the page in an off-screen window, saved as PNG.
fn snapshot(
    host: &mut Host,
    platform: &Platform,
    args: &Args,
    out: &Path,
    early: Vec<Command>,
    rx: &mpsc::Receiver<Option<Command>>,
) -> Result<()> {
    let (w, h) = args
        .size
        .split_once(['x', 'X'])
        .and_then(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?)))
        .context("--size must be WxH")?;
    let window = platform.create_offscreen(w, h)?;
    let output = OutputInfo {
        id: lumen_platform::OutputId("offscreen".into()),
        name: String::new(),
        manufacturer: String::new(),
        product_code: 0,
        device: "offscreen".into(),
        x: -32_000,
        y: -32_000,
        width: w,
        height: h,
        refresh_mhz: 0,
        primary: false,
    };
    let screen = host.page(window, &output)?;
    let screens = [screen];
    for c in early {
        apply(host, &screens, platform, c);
    }
    // Piped commands still apply (scripted tests of the API).
    let until = std::time::Instant::now() + Duration::from_secs_f64(args.after.max(0.0));
    while std::time::Instant::now() < until {
        platform.poll_events(Duration::from_millis(16));
        while let Ok(Some(c)) = rx.try_recv() {
            apply(host, &screens, platform, c);
        }
    }
    let [screen] = screens;
    let png = capture(&screen.webview, platform)?;
    std::fs::write(out, png).with_context(|| format!("writing {}", out.display()))?;
    println!("wrote {} ({w}×{h})", out.display());
    Ok(())
}

/// PNG of what the WebView shows (WebView2 `CapturePreview`).
#[cfg(windows)]
fn capture(webview: &WebView, platform: &Platform) -> Result<Vec<u8>> {
    use webview2_com::CapturePreviewCompletedHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG;
    use windows::Win32::System::Com::{STATFLAG_NONAME, STREAM_SEEK_SET};
    use windows::Win32::UI::Shell::SHCreateMemStream;
    use wry::WebViewExtWindows;

    // SAFETY: COM calls on the UI thread that owns the WebView; the stream
    // and handler stay alive until the completion message is pumped.
    unsafe {
        let stream = SHCreateMemStream(None).context("creating a memory stream")?;
        let (tx, rx) = mpsc::channel();
        let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
            let _ = tx.send(result.is_ok());
            Ok(())
        }));
        webview
            .webview()
            .CapturePreview(
                COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
                &stream,
                &handler,
            )
            .context("CapturePreview")?;
        let done = loop {
            platform.poll_events(Duration::from_millis(16));
            match rx.try_recv() {
                Ok(ok) => break ok,
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => break false,
            }
        };
        anyhow::ensure!(done, "capture failed");
        let mut stat = Default::default();
        stream
            .Stat(&mut stat, STATFLAG_NONAME)
            .context("stream size")?;
        stream
            .Seek(0, STREAM_SEEK_SET, None)
            .context("stream seek")?;
        let mut png = vec![0u8; usize::try_from(stat.cbSize).unwrap_or(0)];
        let mut read = 0u32;
        stream
            .Read(png.as_mut_ptr().cast(), png.len() as u32, Some(&mut read))
            .ok()
            .context("reading the capture")?;
        png.truncate(read as usize);
        Ok(png)
    }
}

#[cfg(not(windows))]
fn capture(_webview: &WebView, _platform: &Platform) -> Result<Vec<u8>> {
    anyhow::bail!("snapshots need WebView2 (Windows)")
}

fn build(host: &mut Host, platform: &Platform, span: bool) -> Result<Vec<Screen>> {
    let mut outputs = platform.outputs()?;
    if span {
        outputs = span_output(&outputs).into_iter().collect();
    }
    let mut screens = Vec::new();
    for output in outputs {
        match host.screen(platform, &output) {
            Ok(s) => screens.push(s),
            Err(e) => tracing::warn!(output = %output.device, "web wallpaper: {e:#}"),
        }
    }
    anyhow::ensure!(
        !screens.is_empty(),
        "no output could show the web wallpaper"
    );
    Ok(screens)
}

fn apply(host: &mut Host, screens: &[Screen], platform: &Platform, command: Command) {
    let js = match command {
        Command::Properties(choices) => {
            let project = properties::read_project(&host.root);
            host.properties = properties::user_properties(&project, &choices);
            format!("__lumen.properties({})", host.properties)
        }
        Command::General(g) => {
            host.general = g;
            format!("__lumen.general({})", host.general)
        }
        Command::Paused(p) => {
            match p {
                Pause::All(all) => {
                    host.paused.clear();
                    host.paused_all = all;
                }
                Pause::Outputs(map) => host.paused.extend(map),
            }
            for s in screens {
                let js = format!(
                    "window.__lumen&&__lumen.paused({})",
                    host.is_paused(&s.output)
                );
                let _ = s.webview.evaluate_script(&js);
            }
            return;
        }
        Command::Audio(levels) => format!("__lumen.audio({})", Value::from(levels)),
        Command::Media(kind, event) => format!("__lumen.media({},{event})", Value::from(kind)),
        Command::Snapshot(path) => {
            save_snapshot(screens, platform, &path);
            return;
        }
        Command::Pointer(x, y, kind) => {
            // Only the output under the cursor hears it, in its own CSS pixels.
            for s in screens {
                let (w, h) = s.window.size();
                let (rx, ry) = (x - s.output.x, y - s.output.y);
                if rx >= 0 && ry >= 0 && (rx as u32) < w && (ry as u32) < h {
                    let js = format!(
                        "__lumen.pointer({rx}/devicePixelRatio,{ry}/devicePixelRatio,{})",
                        Value::from(kind.as_str())
                    );
                    let _ = s.webview.evaluate_script(&js);
                }
            }
            return;
        }
    };
    let js = format!("window.__lumen&&{js}");
    for s in screens {
        if let Err(e) = s.webview.evaluate_script(&js) {
            tracing::debug!("web wallpaper script: {e}");
        }
    }
}

pub fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    if let Err(e) = run(Args::parse()) {
        tracing::error!("{e:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_parse() {
        assert!(matches!(
            parse(r#"{"paused": true}"#),
            Some(Command::Paused(Pause::All(true)))
        ));
        assert!(matches!(
            parse(r#"{"paused": {"a": true, "b": false}}"#),
            Some(Command::Paused(Pause::Outputs(m))) if m["a"] && !m["b"]
        ));
        assert!(matches!(
            parse(r#"{"audio": [0.5, 1]}"#),
            Some(Command::Audio(a)) if a == [0.5, 1.0]
        ));
        assert!(matches!(
            parse(r#"{"media": {"kind": "playback", "event": {"state": 1}}}"#),
            Some(Command::Media(k, e)) if k == "playback" && e["state"] == 1
        ));
        assert!(parse("not json").is_none());
        assert!(parse(r#"{"unknown": 1}"#).is_none());
    }

    #[test]
    fn span_covers_every_output() {
        let out = |x, y, w, h| OutputInfo {
            id: lumen_platform::OutputId(String::new()),
            name: String::new(),
            manufacturer: String::new(),
            product_code: 0,
            device: String::new(),
            x,
            y,
            width: w,
            height: h,
            refresh_mhz: 0,
            primary: false,
        };
        // This machine: 1920×1080 at 0,0 and a portrait 1080×1920 at −1080,−546.
        let s = span_output(&[out(0, 0, 1920, 1080), out(-1080, -546, 1080, 1920)]).unwrap();
        assert_eq!((s.x, s.y, s.width, s.height), (-1080, -546, 3000, 1920));
        assert!(span_output(&[]).is_none());
    }

    #[test]
    fn hosts_differ_per_folder() {
        assert_ne!(host_name(Path::new("C:/a")), host_name(Path::new("C:/b")));
        assert_eq!(host_name(Path::new("C:/A")), host_name(Path::new("c:/a")));
        assert_eq!(wry_origin("lumen://w1"), "https://lumen.w1");
    }
}
