//! The editor's file work and its preview (ADR-009): `dioramad --preview`
//! draws the wallpaper being edited; its frames reach the page through the
//! `preview` protocol, what it says through the `preview` event.

use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use diorama_ipc::preview::{self, Event, Message};
use diorama_project::{Effect, edit, package};
use serde::Serialize;
use serde_json::Value;
use tauri::http::{Request, Response, StatusCode};
use tauri::{AppHandle, Emitter, Manager, State, UriSchemeResponder};

use crate::{blocking, daemon_exe};

/// The event the page listens to.
const EVENT: &str = "preview";

/// The running preview.
#[derive(Default)]
pub struct Previews(Mutex<Option<Preview>>);

struct Preview {
    child: Child,
    input: ChildStdin,
    frames: Arc<Frames>,
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.frames.end();
        // It ends with its standard input too, but may be busy.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The newest frame, and whoever waits for a newer one.
#[derive(Default)]
struct Frames(Mutex<Newest>);

#[derive(Default)]
struct Newest {
    serial: u32,
    width: u32,
    height: u32,
    rgba: Arc<Vec<u8>>,
    waiting: Option<UriSchemeResponder>,
    ended: bool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Newest {
    /// Serial, width and height (little-endian), then the pixels.
    fn body(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(12 + self.rgba.len());
        for n in [self.serial, self.width, self.height] {
            body.extend(n.to_le_bytes());
        }
        body.extend_from_slice(&self.rgba);
        body
    }
}

impl Frames {
    fn push(&self, width: u32, height: u32, rgba: Vec<u8>) {
        let mut newest = lock(&self.0);
        newest.serial = newest.serial.wrapping_add(1).max(1);
        (newest.width, newest.height) = (width, height);
        newest.rgba = Arc::new(rgba);
        if let Some(responder) = newest.waiting.take() {
            responder.respond(answer(StatusCode::OK, newest.body()));
        }
    }

    /// Answers with a frame other than `seen`, now or when there is one.
    fn wait(&self, seen: u32, responder: UriSchemeResponder) {
        let mut newest = lock(&self.0);
        if newest.ended {
            responder.respond(answer(StatusCode::GONE, Vec::new()));
        } else if newest.serial != seen {
            responder.respond(answer(StatusCode::OK, newest.body()));
        } else if let Some(older) = newest.waiting.replace(responder) {
            older.respond(answer(StatusCode::NO_CONTENT, Vec::new()));
        }
    }

    fn end(&self) {
        let mut newest = lock(&self.0);
        newest.ended = true;
        if let Some(responder) = newest.waiting.take() {
            responder.respond(answer(StatusCode::GONE, Vec::new()));
        }
    }
}

fn answer(status: StatusCode, body: Vec<u8>) -> Response<Vec<u8>> {
    let mut response = Response::new(body);
    *response.status_mut() = status;
    let headers = response.headers_mut();
    for (name, value) in [
        ("content-type", "application/octet-stream"),
        ("cache-control", "no-store"),
        ("access-control-allow-origin", "*"),
    ] {
        headers.insert(name, tauri::http::HeaderValue::from_static(value));
    }
    response
}

/// The `preview` protocol: `frame?after=<serial>`.
pub fn serve(app: &AppHandle, request: &Request<Vec<u8>>, responder: UriSchemeResponder) {
    let seen = request
        .uri()
        .query()
        .and_then(|q| q.strip_prefix("after="))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    let previews = app.state::<Previews>();
    let frames = lock(&previews.0).as_ref().map(|p| Arc::clone(&p.frames));
    match frames {
        Some(frames) => frames.wait(seen, responder),
        None => responder.respond(answer(StatusCode::GONE, Vec::new())),
    }
}

/// Starts the preview of the wallpaper in `dir` (instead of another).
#[tauri::command]
pub async fn preview_start(app: AppHandle, dir: PathBuf) -> Result<(), String> {
    blocking(move || {
        let mut command = Command::new(daemon_exe());
        diorama_platform::no_console(&mut command);
        let mut child = command
            .arg("--preview")
            .arg(&dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("starting the preview: {e}"))?;
        let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
            return Err("the preview has no pipes".into());
        };
        let frames = Arc::new(Frames::default());
        let shown = Arc::clone(&frames);
        let handle = app.clone();
        std::thread::spawn(move || {
            let mut output = BufReader::with_capacity(1 << 16, output);
            loop {
                match preview::read(&mut output) {
                    Ok(Some(Message::Frame {
                        width,
                        height,
                        rgba,
                    })) => shown.push(width, height, rgba),
                    Ok(Some(Message::Event(event))) => {
                        let _ = handle.emit(EVENT, event);
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let message = e.to_string();
                        let _ = handle.emit(EVENT, Event::Error { message });
                        break;
                    }
                }
            }
            shown.end();
        });
        let previews = app.state::<Previews>();
        let preview = Preview {
            child,
            input,
            frames,
        };
        let before = lock(&previews.0).replace(preview);
        drop(before);
        Ok(())
    })
    .await
}

/// Passes `command` on to the preview.
#[tauri::command]
pub fn preview_command(
    previews: State<'_, Previews>,
    command: preview::Command,
) -> Result<(), String> {
    let mut line = serde_json::to_vec(&command).map_err(|e| e.to_string())?;
    line.push(b'\n');
    match lock(&previews.0).as_mut() {
        Some(p) => p
            .input
            .write_all(&line)
            .and_then(|()| p.input.flush())
            .map_err(|e| format!("the preview ended: {e}")),
        None => Err("no preview".into()),
    }
}

#[tauri::command]
pub async fn preview_stop(app: AppHandle) -> Result<(), String> {
    blocking(move || {
        let preview = lock(&app.state::<Previews>().0).take();
        drop(preview);
        Ok(())
    })
    .await
}

/// Where new and received wallpapers go: `Documents/Diorama`.
fn wallpapers_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let documents = app.path().document_dir().map_err(|e| e.to_string())?;
    Ok(documents.join("Diorama"))
}

#[derive(Serialize)]
pub struct Made {
    /// The new wallpaper.
    dir: PathBuf,
    /// The folder it is in, for the library.
    folder: PathBuf,
}

#[tauri::command]
pub async fn editor_create(app: AppHandle, title: String, size: [u32; 2]) -> Result<Made, String> {
    let folder = wallpapers_dir(&app)?;
    blocking(move || {
        let dir = edit::create(&folder, &title, size).map_err(|e| e.to_string())?;
        Ok(Made { dir, folder })
    })
    .await
}

/// Unpacks a `.diorama` file.
#[tauri::command]
pub async fn editor_import(app: AppHandle, file: PathBuf) -> Result<Made, String> {
    let folder = wallpapers_dir(&app)?;
    blocking(move || {
        let dir = package::import(&file, &folder).map_err(|e| e.to_string())?;
        Ok(Made { dir, folder })
    })
    .await
}

/// Writes the wallpaper in `dir` to a `.diorama` file.
#[tauri::command]
pub async fn editor_export(dir: PathBuf, file: PathBuf) -> Result<(), String> {
    blocking(move || package::export(&dir, &file).map_err(|e| e.to_string())).await
}

#[derive(Serialize)]
pub struct Opened {
    document: Value,
    project: Value,
}

/// The wallpaper in `dir`, whose images the page may show from now on.
#[tauri::command]
pub async fn editor_open(app: AppHandle, dir: PathBuf) -> Result<Opened, String> {
    blocking(move || {
        let (document, project) = edit::open(&dir).map_err(|e| e.to_string())?;
        app.asset_protocol_scope()
            .allow_directory(&dir, true)
            .map_err(|e| e.to_string())?;
        Ok(Opened { document, project })
    })
    .await
}

#[tauri::command]
pub async fn editor_save(dir: PathBuf, document: Value) -> Result<(), String> {
    blocking(move || edit::save(&dir, &document).map_err(|e| e.to_string())).await
}

#[tauri::command]
pub async fn editor_save_project(dir: PathBuf, project: Value) -> Result<(), String> {
    blocking(move || edit::save_project(&dir, &project).map_err(|e| e.to_string())).await
}

/// Copies a file into the wallpaper; its path there.
#[tauri::command]
pub async fn editor_add_asset(dir: PathBuf, source: PathBuf) -> Result<String, String> {
    blocking(move || edit::add_asset(&dir, &source).map_err(|e| e.to_string())).await
}

/// Families of the fonts installed here.
#[tauri::command]
pub async fn editor_fonts() -> Result<Vec<String>, String> {
    blocking(|| Ok(diorama_platform::font_families())).await
}

/// Copies an installed font into the wallpaper; its path there.
#[tauri::command]
pub async fn editor_add_font(
    dir: PathBuf,
    family: String,
    bold: bool,
    italic: bool,
) -> Result<String, String> {
    blocking(move || {
        let face = diorama_platform::font_face(&family, bold, italic)
            .ok_or_else(|| format!("{family} is not installed"))?;
        if face.index != 0 {
            return Err("this font shares its file with others and cannot be copied alone".into());
        }
        edit::add_asset(&dir, &face.path).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub fn editor_effects() -> &'static [Effect] {
    diorama_project::effects()
}

/// Saves the preview's newest frame as the wallpaper's preview picture.
#[tauri::command]
pub async fn editor_picture(app: AppHandle, dir: PathBuf) -> Result<(), String> {
    blocking(move || {
        let frames = lock(&app.state::<Previews>().0)
            .as_ref()
            .map(|p| Arc::clone(&p.frames))
            .ok_or("no preview")?;
        let (rgba, width, height) = {
            let newest = lock(&frames.0);
            (Arc::clone(&newest.rgba), newest.width, newest.height)
        };
        if rgba.is_empty() {
            return Err("nothing drawn yet".into());
        }
        edit::save_preview(&dir, &rgba, width, height).map_err(|e| e.to_string())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_say_their_size() {
        let frames = Frames::default();
        frames.push(2, 1, vec![9; 8]);
        let newest = lock(&frames.0);
        let body = newest.body();
        assert_eq!(body.len(), 20);
        assert_eq!(body[..12], [1, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0]);
    }
}
