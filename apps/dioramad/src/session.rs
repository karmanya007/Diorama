//! Windows, surfaces and GPU resources for the current desktop + wallpaper.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use diorama_gpu::{Gpu, OutputSurface};
use diorama_ipc::{Fit, Layout, MediaKind};
use diorama_media::{Nv12Frame, VideoPlayer};
use diorama_platform::{Coverage, OutputInfo, Platform, WallpaperWindow};
use diorama_render::scene::{FrameInputs, SceneRenderer};
use diorama_render::{
    FitMode, ImageBlit, Nv12Planes, Rect, VideoBlit, display_size, render_size, uv_transform,
};

/// How the picture is placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    pub fit: Fit,
    pub layout: Layout,
}

impl Look {
    fn fit_mode(self) -> FitMode {
        match self.fit {
            Fit::Fill => FitMode::Fill,
            Fit::Fit => FitMode::Fit,
            Fit::Stretch => FitMode::Stretch,
            Fit::Custom { x, y, zoom } => FitMode::Custom { x, y, zoom },
            Fit::Native { x, y } => FitMode::Native { x, y },
        }
    }
}

/// Decoded content; lives across desktop rebuilds.
pub enum Content {
    Image(image::RgbaImage),
    Video(VideoPlayer),
    /// A scene project. Its imported form (decoded textures, often hundreds
    /// of MB) is only held until the GPU has it; a rebuild re-imports.
    Scene(SceneSource),
    /// A web page, shown by a `diorama-webhost` process.
    Web(crate::web::WebSource),
}

pub struct SceneSource {
    dir: PathBuf,
    we_assets: Option<PathBuf>,
    /// Property values: WE's choices and the user's overrides.
    choices: crate::properties::Values,
    imported: std::cell::RefCell<Option<Box<diorama_scene::Scene>>>,
}

impl SceneSource {
    /// The imported scene: the copy from loading if still held, else a fresh import.
    fn take(&self) -> Result<Box<diorama_scene::Scene>> {
        match self.imported.borrow_mut().take() {
            Some(scene) => Ok(scene),
            None => Ok(Box::new(crate::headless::load_scene(
                &self.dir,
                self.we_assets.as_deref(),
                &self.choices,
            )?)),
        }
    }
}

/// The wallpaper the user asked for.
pub struct Wallpaper {
    pub path: PathBuf,
    pub kind: MediaKind,
    pub look: Look,
    pub content: Content,
    /// Videos: what decodes and plays.
    pub decode: Decode,
}

/// How a video plays: whether the CPU decodes it, and whether a copy
/// converted for hardware decoding plays (P4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Decode {
    pub software: bool,
    pub converted: bool,
}

/// The path a wallpaper is known by (and its overrides kept under): the
/// project folder for scenes and web pages, else the file.
pub fn wallpaper_path(path: &Path, kind: Option<MediaKind>) -> Option<PathBuf> {
    Some(match kind.or_else(|| MediaKind::from_path(path))? {
        MediaKind::Scene => scene_dir(path),
        MediaKind::Web if !path.is_dir() => path.parent()?.to_path_buf(),
        MediaKind::Video if path.is_dir() => project_video(path)?,
        _ => path.to_path_buf(),
    })
}

/// The video a WE video project folder plays (its `file`, inside the folder).
fn project_video(dir: &Path) -> Option<PathBuf> {
    let file = crate::properties::project(dir)["file"].as_str()?.to_owned();
    let rel = Path::new(&file);
    if !rel
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    let path = dir.join(rel);
    path.is_file().then_some(path)
}

/// A scene project folder from the folder itself or one of its files.
fn scene_dir(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .map_or_else(|| path.to_path_buf(), Path::to_path_buf)
    }
}

impl Wallpaper {
    /// `overrides`: property values chosen in Diorama, over WE's choices.
    pub fn load(
        path: &Path,
        kind: Option<MediaKind>,
        look: Look,
        we_assets: Option<&Path>,
        we_config: Option<&Path>,
        overrides: &crate::properties::Values,
    ) -> Result<Self> {
        let kind = kind
            .or_else(|| MediaKind::from_path(path))
            .with_context(|| format!("unknown media type for {}", path.display()))?;
        let path = &match kind {
            MediaKind::Scene => scene_dir(path),
            MediaKind::Video if path.is_dir() => project_video(path)
                .with_context(|| format!("no video file in {}", path.display()))?,
            _ => path.to_path_buf(),
        };
        if kind == MediaKind::Web {
            let source = crate::web::WebSource::new(path, we_config, overrides.clone())?;
            return Ok(Self {
                path: source.dir.clone(),
                kind,
                look,
                content: Content::Web(source),
                decode: Decode::default(),
            });
        }
        let mut decode = Decode::default();
        let content = match kind {
            MediaKind::Image => Content::Image(
                image::open(path)
                    .with_context(|| format!("loading {}", path.display()))?
                    .into_rgba8(),
            ),
            MediaKind::Video => {
                // A copy converted for hardware decoding plays instead, if made.
                let copy = crate::convert::existing_copy(path);
                let play = copy.as_deref().unwrap_or(path);
                decode = Decode {
                    software: diorama_media::probe(play).is_ok_and(|p| !p.hardware),
                    converted: copy.is_some(),
                };
                if decode.software {
                    tracing::info!(video = %play.display(), "no hardware decoder; the CPU decodes it");
                }
                Content::Video(
                    VideoPlayer::spawn(play)
                        .with_context(|| format!("opening {}", play.display()))?,
                )
            }
            // Import now so a broken scene is reported by `diorama set`.
            MediaKind::Web => unreachable!("handled above"),
            MediaKind::Scene => {
                let choices = crate::properties::choices(we_config, path, overrides);
                let scene = crate::headless::load_scene(path, we_assets, &choices)?;
                Content::Scene(SceneSource {
                    dir: path.to_path_buf(),
                    we_assets: we_assets.map(Path::to_path_buf),
                    choices,
                    imported: std::cell::RefCell::new(Some(Box::new(scene))),
                })
            }
        };
        Ok(Self {
            path: path.to_path_buf(),
            kind,
            look,
            content,
            decode,
        })
    }
}

enum Renderer {
    Image(ImageBlit),
    Video(VideoBlit),
    Scene {
        scene: Box<SceneRenderer>,
        scripts: Option<Box<crate::scripts::SceneScripts>>,
        /// What the system plays, for audio-reactive scenes.
        audio: Option<SceneAudio>,
        /// Players feeding the scene's video textures.
        videos: Option<crate::videos::SceneVideos>,
        /// The scene's sound objects.
        sounds: Option<crate::sounds::SceneSounds>,
        blit: ImageBlit,
        /// Fills monitor areas the scene does not cover (fit/custom pan), as WE.
        clear: [f32; 4],
    },
    /// The page draws itself in its own process and windows.
    Web(Box<crate::web::WebHost>),
}

/// Loopback capture of the system's audio for a scene, reopened when the
/// default output device changes.
pub struct SceneAudio {
    capture: Option<diorama_media::AudioCapture>,
    last_time: Option<f32>,
    next_check: f32,
    /// Loudest band since the last check (debug log: is the scene hearing anything?).
    peak: f32,
}

impl SceneAudio {
    pub fn start() -> Option<Self> {
        let capture = match diorama_media::AudioCapture::start() {
            Ok(c) => {
                tracing::info!("audio capture started (default output, loopback)");
                Some(c)
            }
            Err(e) => {
                tracing::info!("audio capture unavailable: {e}");
                None
            }
        };
        Some(Self {
            capture,
            last_time: None,
            next_check: 2.0,
            peak: 0.0,
        })
    }

    /// (left, right) 64-band spectrum at scene time `time`.
    pub fn spectrum(&mut self, time: f32) -> [[f32; 64]; 2] {
        let dt = self.last_time.map_or(0.0, |t| (time - t).max(0.0));
        self.last_time = Some(time);
        self.check(time);
        let spectrum = self.capture.as_ref().map_or([[0.0; 64]; 2], |c| {
            let s = c.spectrum(dt);
            [s.left, s.right]
        });
        self.peak = spectrum.iter().flatten().fold(self.peak, |m, &v| m.max(v));
        spectrum
    }

    /// (left, right) 64 bands as WE gives web wallpapers: linear levels,
    /// typically well under 0.1, not the normalised scene spectrum.
    pub fn web_spectrum(&mut self, time: f32) -> [[f32; 64]; 2] {
        self.check(time);
        let spectrum = self.capture.as_ref().map_or([[0.0; 64]; 2], |c| {
            let s = c.web_spectrum();
            [s.left, s.right]
        });
        self.peak = spectrum.iter().flatten().fold(self.peak, |m, &v| m.max(v));
        spectrum
    }

    /// Every 2 s: log what was heard and reopen a failed or stale capture.
    fn check(&mut self, time: f32) {
        if time >= self.next_check {
            self.next_check = time + 2.0;
            tracing::debug!(
                peak = self.peak,
                capturing = self.capture.is_some(),
                "audio spectrum (loudest band, last 2 s)"
            );
            self.peak = 0.0;
            if self.capture.as_ref().is_none_or(|c| c.needs_reopen()) {
                self.capture = diorama_media::AudioCapture::start().ok();
            }
        }
    }
}

/// One monitor's wallpaper. Field order matters: the surface must drop before its window.
pub struct Output {
    pub info: OutputInfo,
    /// Not receiving new frames (covered, locked, …); the last frame stays on screen.
    pub paused: bool,
    pub coverage: Coverage,
    /// `None` for web wallpapers, which draw in their own windows.
    surface: Option<OutputSurface>,
    window: Option<WallpaperWindow>,
}

impl Output {
    /// See [`WallpaperWindow::keep_in_place`]. True if the window moved.
    pub fn keep_in_place(&self) -> bool {
        self.window
            .as_ref()
            .is_some_and(WallpaperWindow::keep_in_place)
    }
}

pub struct Session {
    pub outputs: Vec<Output>,
    canvas: Rect,
    renderer: Renderer,
    look: Look,
    /// Size the content is fitted by (scenes: the scene size).
    fit_size: (u32, u32),
    /// Size of the area the cursor was last mapped over (see [`Self::pointer`]).
    pointer_area: std::cell::Cell<(u32, u32)>,
}

/// Fraction `p` of `area` → fraction of content of `size` fitted to it with `mode`.
fn content_point(mode: FitMode, size: (u32, u32), area: Rect, p: [f32; 2]) -> [f32; 2] {
    if size.0 == 0 || size.1 == 0 {
        return p;
    }
    let (s, o) = uv_transform(mode, size, area, area);
    [p[0] * s[0] + o[0], p[1] * s[1] + o[1]]
}

pub fn rect_of(o: &OutputInfo) -> Rect {
    Rect {
        x: o.x,
        y: o.y,
        width: o.width,
        height: o.height,
    }
}

impl Session {
    /// Shows `wallpaper` on the monitors `infos`. `below`: its windows go
    /// under a wallpaper being replaced until [`Self::raise`] (web: until the
    /// page has painted).
    pub fn build(
        platform: &Platform,
        gpu: &Gpu,
        wallpaper: &Wallpaper,
        infos: Vec<OutputInfo>,
        web_network: bool,
        below: bool,
    ) -> Result<Self> {
        let look = wallpaper.look;
        let canvas = Rect::bounding(infos.iter().map(rect_of)).context("no outputs found")?;
        if let Content::Web(source) = &wallpaper.content {
            // The host makes its own windows; outputs here only track pauses.
            let ids: Vec<String> = infos.iter().map(|o| o.id.0.clone()).collect();
            let host = crate::web::WebHost::spawn(
                source,
                web_network,
                look.layout == Layout::Span,
                &ids,
                below,
                platform.waker(),
            )?;
            let outputs = infos
                .into_iter()
                .map(|info| Output {
                    info,
                    paused: false,
                    coverage: Coverage::Visible,
                    surface: None,
                    window: None,
                })
                .collect::<Vec<_>>();
            tracing::info!(outputs = outputs.len(), "web wallpaper shown");
            return Ok(Self {
                outputs,
                canvas,
                renderer: Renderer::Web(Box::new(host)),
                look,
                fit_size: (0, 0),
                pointer_area: std::cell::Cell::new((1, 1)),
            });
        }

        let mut outputs = Vec::new();
        for info in infos {
            let window = if below {
                platform.create_wallpaper_below(&info)
            } else {
                platform.create_wallpaper(&info)
            }
            .with_context(|| format!("wallpaper window for {}", info.device))?;
            let handles = window.handles();
            // SAFETY: `window` is stored next to the surface in `Output` and dropped after it.
            let surface = unsafe {
                gpu.create_surface(handles.window, handles.display, info.width, info.height)
            }
            .with_context(|| format!("GPU surface for {}", info.device))?;
            outputs.push(Output {
                info,
                paused: false,
                coverage: Coverage::Visible,
                surface: Some(surface),
                window: Some(window),
            });
        }
        let format = outputs
            .iter()
            .find_map(|o| o.surface.as_ref().map(OutputSurface::format))
            .context("no output surface")?;

        let renderer = match &wallpaper.content {
            Content::Image(image) => {
                // Downscale once to the largest size the image is ever displayed at (sharp, less VRAM).
                let sizes: Vec<(u32, u32)> = match look.layout {
                    Layout::Span => vec![canvas.size()],
                    Layout::PerOutput => outputs
                        .iter()
                        .map(|o| (o.info.width, o.info.height))
                        .collect(),
                };
                let (w, h) = display_size(look.fit_mode(), image.dimensions(), &sizes);
                let pixels = if (w, h) == image.dimensions() {
                    image.clone()
                } else {
                    image::imageops::resize(image, w, h, image::imageops::FilterType::Lanczos3)
                };
                Renderer::Image(
                    ImageBlit::new(gpu, format, pixels.as_raw(), w, h)
                        .with_fit_size(image.dimensions()),
                )
            }
            Content::Video(player) => {
                let info = player.info();
                Renderer::Video(VideoBlit::new(gpu, format, info.width, info.height))
            }
            Content::Web(_) => unreachable!("handled above"),
            Content::Scene(source) => {
                let scene = source.take()?;
                let compiler = crate::headless::shader_compiler()?;
                // Composite at the size the monitors actually show (P12),
                // like WE rendering straight into its swapchain.
                let sizes: Vec<(u32, u32)> = match look.layout {
                    Layout::Span => vec![canvas.size()],
                    Layout::PerOutput => outputs
                        .iter()
                        .map(|o| (o.info.width, o.info.height))
                        .collect(),
                };
                let max = gpu.device.limits().max_texture_dimension_2d;
                let (w, h) = render_size(look.fit_mode(), (scene.width, scene.height), &sizes, max);
                let renderer = SceneRenderer::new(gpu, &scene, &compiler, Some([w, h]))?;
                let factory = crate::headless::layer_factory(
                    &source.dir,
                    source.we_assets.as_deref(),
                    &source.choices,
                    &scene,
                )
                .map(|f| (f, compiler.clone()));
                let scripts = crate::scripts::SceneScripts::new(
                    &scene,
                    crate::scripts::now_ms(),
                    true,
                    factory,
                    Some(&source.dir),
                )
                .map(Box::new);
                let videos = crate::videos::SceneVideos::start(&scene);
                let sounds = crate::sounds::SceneSounds::new(&scene);
                let [r, g, b] = scene.scheme_color;
                let scene_clear = [r, g, b, 1.0];
                let scene_size = (scene.width, scene.height);
                // CPU copies of textures are not needed once uploaded.
                drop(scene);
                let [w, h] = renderer.size();
                let blit = ImageBlit::from_view(gpu, format, renderer.output(), w, h)
                    .with_fit_size(scene_size);
                Renderer::Scene {
                    scene: Box::new(renderer),
                    scripts,
                    audio: SceneAudio::start(),
                    videos,
                    sounds,
                    blit,
                    clear: scene_clear,
                }
            }
        };

        let fit_size = match &renderer {
            Renderer::Scene { blit, .. } | Renderer::Image(blit) => blit.fit_size(),
            _ => (0, 0),
        };
        let mut session = Self {
            outputs,
            canvas,
            renderer,
            look,
            fit_size,
            pointer_area: std::cell::Cell::new((1, 1)),
        };
        if matches!(session.renderer, Renderer::Image(_)) {
            session.present_all(gpu)?;
        }
        tracing::info!(
            outputs = session.outputs.len(),
            path = %wallpaper.path.display(),
            layout = ?look.layout,
            "wallpaper shown"
        );
        Ok(session)
    }

    pub fn show_video_frame(&mut self, gpu: &Gpu, frame: &Nv12Frame) -> Result<()> {
        if let Renderer::Video(blit) = &self.renderer {
            blit.upload(
                gpu,
                Nv12Planes {
                    width: frame.width,
                    height: frame.height,
                    stride: frame.stride,
                    luma: frame.luma(),
                    chroma: frame.chroma(),
                },
            );
        }
        self.present_all(gpu)
    }

    /// Cursor as a fraction of what the scene covers (x right, y down): the
    /// span canvas, or the monitor under the cursor in per-output layout.
    /// `None` when the cursor is not over any output.
    pub fn pointer(&self, cursor: (i32, i32)) -> Option<[f32; 2]> {
        let area = match self.look.layout {
            Layout::Span => self.canvas,
            Layout::PerOutput => self.outputs.iter().map(|o| rect_of(&o.info)).find(|r| {
                cursor.0 >= r.x
                    && cursor.1 >= r.y
                    && i64::from(cursor.0) < i64::from(r.x) + i64::from(r.width)
                    && i64::from(cursor.1) < i64::from(r.y) + i64::from(r.height)
            })?,
        };
        self.pointer_area.set(area.size());
        Some([
            (cursor.0 - area.x) as f32 / area.width.max(1) as f32,
            (cursor.1 - area.y) as f32 / area.height.max(1) as f32,
        ])
    }

    /// A [`Self::pointer`] fraction as a fraction of the content itself,
    /// through the fit (cropping, zoom, pan): where the cursor is in the
    /// scene for scripts.
    fn content_point(&self, p: [f32; 2]) -> [f32; 2] {
        let (w, h) = self.pointer_area.get();
        let area = Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        content_point(self.look.fit_mode(), self.fit_size, area, p)
    }

    /// Renders the scene at `time` and presents it (no-op for other content).
    /// `buttons`: mouse state when the cursor is over this wallpaper.
    /// The scene's sound objects, if it has any.
    pub fn scene_sounds(&mut self) -> Option<&mut crate::sounds::SceneSounds> {
        match &mut self.renderer {
            Renderer::Scene { sounds, .. } => sounds.as_mut(),
            _ => None,
        }
    }

    pub fn show_scene_frame(
        &mut self,
        gpu: &Gpu,
        time: f32,
        pointer: [f32; 2],
        buttons: Option<diorama_platform::DesktopPointer>,
    ) -> Result<()> {
        // Parallax follows the cursor over the output; scripts get scene points.
        let scene_pointer = self.content_point(pointer);
        if let Renderer::Scene {
            scene,
            scripts,
            audio,
            videos,
            sounds,
            ..
        } = &mut self.renderer
        {
            if let Some(videos) = videos {
                videos.frame(gpu, scene, time);
            }
            let spectrum = audio.as_mut().map_or([[0.0; 64]; 2], |a| a.spectrum(time));
            if let Some(scripts) = scripts {
                let frame = crate::scripts::ScriptFrame {
                    time,
                    clock_ms: crate::scripts::now_ms(),
                    pointer: scene_pointer,
                    buttons,
                    audio: spectrum,
                };
                scripts.frame(gpu, scene, sounds.as_mut(), &frame);
            }
            scene.render(
                gpu,
                &FrameInputs {
                    time,
                    pointer,
                    daytime: diorama_platform::day_fraction(),
                    audio: spectrum,
                },
            );
            self.present_all(gpu)?;
        }
        Ok(())
    }

    /// Puts its windows above any wallpaper being replaced.
    pub fn raise(&self) {
        for o in &self.outputs {
            if let Some(w) = &o.window {
                w.raise();
            }
        }
    }

    /// Process id of the web wallpaper host, if this session shows one.
    pub fn web_pid(&self) -> Option<u32> {
        match &self.renderer {
            Renderer::Web(host) => host.pid(),
            _ => None,
        }
    }

    /// What the web wallpaper host reported about its page.
    pub fn web_report(&self) -> Option<crate::web::PageReport> {
        match &self.renderer {
            Renderer::Web(host) => Some(host.report()),
            _ => None,
        }
    }

    /// The web wallpaper host, if this session shows one.
    pub fn web(&mut self) -> Option<&mut crate::web::WebHost> {
        match &mut self.renderer {
            Renderer::Web(host) => Some(host),
            _ => None,
        }
    }

    /// The last rendered scene frame, for the OS fallback wallpaper.
    pub fn scene_snapshot(&self, gpu: &Gpu) -> Option<image::RgbaImage> {
        let Renderer::Scene { scene, .. } = &self.renderer else {
            return None;
        };
        let [w, h] = scene.size();
        image::RgbaImage::from_raw(w, h, scene.read_output(gpu)?)
    }

    /// Draws the current content on every output that is not paused.
    pub fn present_all(&mut self, gpu: &Gpu) -> Result<()> {
        let fit = self.look.fit_mode();
        for out in self.outputs.iter_mut().filter(|o| !o.paused) {
            let target = rect_of(&out.info);
            let canvas = match self.look.layout {
                Layout::Span => self.canvas,
                Layout::PerOutput => target,
            };
            let Some(surface) = out.surface.as_mut() else {
                continue;
            };
            let Some(frame) = surface.acquire(gpu)? else {
                tracing::debug!(output = %out.info.device, "frame skipped (occluded)");
                continue;
            };
            let view = frame
                .texture
                .create_view(&diorama_gpu::wgpu::TextureViewDescriptor::default());
            let clear = match &self.renderer {
                Renderer::Scene { clear, .. } => *clear,
                _ => [0.0, 0.0, 0.0, 1.0],
            };
            match &self.renderer {
                Renderer::Image(blit) => blit.draw(gpu, &view, canvas, target, fit, clear),
                Renderer::Video(blit) => blit.draw(gpu, &view, canvas, target, fit, clear),
                Renderer::Scene { blit, .. } => blit.draw(gpu, &view, canvas, target, fit, clear),
                Renderer::Web(_) => {}
            }
            gpu.queue.present(frame);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_maps_through_the_fit() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 1080,
            height: 1920,
        };
        // A 16:9 scene filling a portrait monitor is cropped at the sides:
        // the monitor's centre is the scene's centre, its edges are inside.
        let fill = |p| content_point(FitMode::Fill, (1920, 1080), area, p);
        let [cx, cy] = fill([0.5, 0.5]);
        assert!((cx - 0.5).abs() < 1e-4 && (cy - 0.5).abs() < 1e-4);
        let [left, _] = fill([0.0, 0.5]);
        assert!(
            (left - (0.5 - 0.5 * (1080.0 / 1920.0) / (1920.0 / 1080.0))).abs() < 1e-3,
            "{left}"
        );
        // Stretch maps 1:1.
        let [sx, sy] = content_point(FitMode::Stretch, (1920, 1080), area, [0.25, 0.75]);
        assert!((sx - 0.25).abs() < 1e-4 && (sy - 0.75).abs() < 1e-4);
        // Nothing to fit: unchanged.
        assert_eq!(
            content_point(FitMode::Fill, (0, 0), area, [0.3, 0.4]),
            [0.3, 0.4]
        );
    }
}
