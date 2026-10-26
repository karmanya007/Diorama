//! IPC between `lumend` and its clients (L1).
//!
//! Transport: a per-user local socket (named pipe on Windows, Unix socket
//! elsewhere) via `interprocess`. Protocol: one JSON request line, one JSON
//! response line, per connection. Every message carries [`PROTOCOL_VERSION`];
//! mismatches are rejected so CLI/UI and daemon never silently misread each other.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{ConnectOptions, GenericNamespaced, ListenerOptions, Name};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// Longest accepted message line; protects the daemon from a client sending garbage forever.
const MAX_LINE: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("lumend is not running")]
    NotRunning,
    #[error("lumend is already running")]
    AlreadyRunning,
    #[error("protocol version mismatch: ours {ours}, theirs {theirs}")]
    Version { ours: u32, theirs: u32 },
    #[error("bad message: {0}")]
    Protocol(String),
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Fit {
    Fill,
    Fit,
    Stretch,
    /// Wallpaper Engine custom alignment: pan x/y and zoom in percent.
    Custom {
        x: i32,
        y: i32,
        zoom: i32,
    },
    /// Native size (one image pixel per screen pixel), panned x/y percent
    /// across what does not fit (WE alignment 3).
    Native {
        x: i32,
        y: i32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Layout {
    PerOutput,
    Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MediaKind {
    Image,
    Video,
    /// A Wallpaper Engine scene project folder.
    Scene,
    /// A web wallpaper: a Wallpaper Engine web project folder or an HTML page.
    Web,
}

impl MediaKind {
    /// Guess from the file extension; scene project folders (or their
    /// `project.json` / `scene.json` / `scene.pkg`) are scenes, web project
    /// folders (`project.json` with `"type": "web"`) and HTML pages are web.
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        if path.is_dir() {
            if project_type(&path.join("project.json")).as_deref() == Some("web") {
                return Some(Self::Web);
            }
            return ["scene.pkg", "scene.json"]
                .iter()
                .any(|f| path.join(f).is_file())
                .then_some(Self::Scene);
        }
        let name = path.file_name()?.to_str()?.to_ascii_lowercase();
        if name == "project.json" && project_type(path).as_deref() == Some("web") {
            return Some(Self::Web);
        }
        if matches!(name.as_str(), "scene.pkg" | "scene.json" | "project.json") {
            return Some(Self::Scene);
        }
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "bmp" | "gif" | "tga" | "webp" => Some(Self::Image),
            "mp4" | "m4v" | "mov" | "webm" | "mkv" | "avi" | "wmv" => Some(Self::Video),
            "html" | "htm" => Some(Self::Web),
            _ => None,
        }
    }
}

/// `type` of a Wallpaper Engine `project.json`, lower-case.
fn project_type(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    Some(v.get("type")?.as_str()?.to_ascii_lowercase())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum Request {
    Status,
    /// Show a new wallpaper. Missing options keep the current ones.
    Set {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<MediaKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fit: Option<Fit>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layout: Option<Layout>,
        /// Web wallpapers: allow (`true`) or deny (`false`) network access for
        /// this wallpaper from now on; absent keeps the remembered choice.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        network: Option<bool>,
        /// Only these monitors (output ids or device names such as
        /// `\\.\DISPLAY1`); the others keep what they show. Absent: every
        /// monitor shows this wallpaper.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outputs: Option<Vec<String>>,
    },
    /// Change property values of a wallpaper (remembered): name → value;
    /// `null` returns a property to WE's choice or the project default.
    /// Showing wallpapers update at once.
    SetProperties {
        path: PathBuf,
        properties: serde_json::Map<String, serde_json::Value>,
    },
    /// A wallpaper's property values as Lumen applies them: the project's
    /// defaults, then WE's choices, then those set in Lumen.
    Properties {
        path: PathBuf,
    },
    /// Video soundtrack volume, 0–100 (0 = muted, no audio device opened).
    Volume {
        percent: u8,
    },
    /// Pause all outputs regardless of policy until `Resume`.
    Pause,
    Resume,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputStatus {
    /// Stable identity (EDID + connector on Windows).
    pub id: String,
    pub device: String,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: f64,
    pub primary: bool,
    pub paused: bool,
    /// `visible`, `maximized` or `fullscreen`.
    pub coverage: String,
    /// What this monitor shows.
    #[serde(default)]
    pub content: Option<ContentStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub pid: u32,
    pub daemon_version: String,
    pub content: Option<ContentStatus>,
    pub user_paused: bool,
    /// Video soundtrack volume, 0–100.
    #[serde(default)]
    pub volume: u8,
    pub outputs: Vec<OutputStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentStatus {
    pub kind: MediaKind,
    pub path: PathBuf,
    pub fit: Fit,
    pub layout: Layout,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "kebab-case")]
pub enum Response {
    Ok,
    Status(Status),
    Properties {
        values: serde_json::Map<String, serde_json::Value>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct Envelope<T> {
    version: u32,
    #[serde(flatten)]
    body: T,
}

fn socket_name() -> std::io::Result<Name<'static>> {
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default();
    let user: String = user
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    format!("lumen-{user}.sock").to_ns_name::<GenericNamespaced>()
}

fn encode<T: Serialize>(body: T) -> Result<Vec<u8>> {
    let mut line = serde_json::to_vec(&Envelope {
        version: PROTOCOL_VERSION,
        body,
    })
    .map_err(|e| Error::Protocol(e.to_string()))?;
    line.push(b'\n');
    Ok(line)
}

fn decode<T: for<'de> Deserialize<'de>>(line: &str) -> Result<T> {
    let env: Envelope<serde_json::Value> =
        serde_json::from_str(line).map_err(|e| Error::Protocol(e.to_string()))?;
    if env.version != PROTOCOL_VERSION {
        return Err(Error::Version {
            ours: PROTOCOL_VERSION,
            theirs: env.version,
        });
    }
    serde_json::from_value(env.body).map_err(|e| Error::Protocol(e.to_string()))
}

fn read_line(reader: &mut impl BufRead) -> Result<String> {
    let mut line = String::new();
    let n = std::io::Read::take(&mut *reader, MAX_LINE as u64).read_line(&mut line)?;
    if n == 0 || !line.ends_with('\n') {
        return Err(Error::Protocol("truncated or oversized message".into()));
    }
    Ok(line)
}

/// Sends one request to the running daemon and waits for its response.
pub fn call(request: &Request) -> Result<Response> {
    let mut stream = ConnectOptions::new()
        .name(socket_name()?)
        .connect_sync()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                Error::NotRunning
            }
            _ => Error::Io(e),
        })?;
    stream.write_all(&encode(request)?)?;
    let mut reader = BufReader::new(stream);
    decode(&read_line(&mut reader)?)
}

/// The daemon's listening end.
pub struct Server {
    listener: interprocess::local_socket::Listener,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server").finish_non_exhaustive()
    }
}

impl Server {
    /// Binds the per-user socket. Fails with [`Error::AlreadyRunning`] if another daemon answers.
    pub fn bind() -> Result<Self> {
        // Named pipes allow several server instances per name, so probe first.
        if ConnectOptions::new()
            .name(socket_name()?)
            .connect_sync()
            .is_ok()
        {
            return Err(Error::AlreadyRunning);
        }
        let listener = ListenerOptions::new()
            .name(socket_name()?)
            .create_sync()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::AddrInUse => Error::AlreadyRunning,
                _ => Error::Io(e),
            })?;
        Ok(Self { listener })
    }

    /// Serves requests forever (run on a dedicated thread). `handler` returns
    /// the response; malformed requests get an error response.
    pub fn serve(self, mut handler: impl FnMut(Request) -> Response) {
        for conn in self.listener.incoming() {
            let Ok(conn) = conn else { continue };
            let mut reader = BufReader::new(conn);
            let response = match read_line(&mut reader).and_then(|l| decode::<Request>(&l)) {
                Ok(request) => handler(request),
                Err(e) => Response::Error {
                    message: e.to_string(),
                },
            };
            if let Ok(bytes) = encode(&response) {
                let _ = reader.get_mut().write_all(&bytes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_as_json_lines() {
        let req = Request::Set {
            path: PathBuf::from("C:/wall.mp4"),
            kind: Some(MediaKind::Video),
            fit: None,
            layout: Some(Layout::Span),
            network: None,
            outputs: Some(vec!["DEL-1234".into()]),
        };
        let line = String::from_utf8(encode(&req).unwrap()).unwrap();
        assert!(line.ends_with('\n'));
        assert!(line.contains(r#""cmd":"set""#), "{line}");
        assert!(line.contains(r#""layout":"span""#), "{line}");
        assert!(line.contains(r#""outputs":["DEL-1234"]"#), "{line}");
        assert!(!line.contains("fit"), "absent options are omitted: {line}");
        assert_eq!(decode::<Request>(&line).unwrap(), req);
    }

    #[test]
    fn version_mismatch_is_rejected() {
        let line = r#"{"version":999,"cmd":"status"}"#;
        assert!(matches!(
            decode::<Request>(line),
            Err(Error::Version { theirs: 999, .. })
        ));
    }

    #[test]
    fn oversized_lines_are_rejected() {
        let big = "x".repeat(MAX_LINE + 10);
        let mut reader = BufReader::new(big.as_bytes());
        assert!(read_line(&mut reader).is_err());
    }

    #[test]
    fn media_kind_from_extension() {
        assert_eq!(
            MediaKind::from_path("a/b.MP4".as_ref()),
            Some(MediaKind::Video)
        );
        assert_eq!(
            MediaKind::from_path("a/b.jpeg".as_ref()),
            Some(MediaKind::Image)
        );
        assert_eq!(MediaKind::from_path("a/b.pkg".as_ref()), None);
        assert_eq!(
            MediaKind::from_path("a/scene.pkg".as_ref()),
            Some(MediaKind::Scene)
        );
        assert_eq!(
            MediaKind::from_path("a/project.json".as_ref()),
            Some(MediaKind::Scene)
        );
        assert_eq!(
            MediaKind::from_path("a/index.HTML".as_ref()),
            Some(MediaKind::Web)
        );
    }

    #[test]
    fn web_projects_by_type() {
        let dir = std::env::temp_dir().join(format!("lumen-ipc-web-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // WE writes project.json with a BOM; `type` is matched case-insensitively.
        std::fs::write(
            dir.join("project.json"),
            "\u{feff}{\"type\": \"Web\", \"file\": \"index.html\"}",
        )
        .unwrap();
        assert_eq!(MediaKind::from_path(&dir), Some(MediaKind::Web));
        assert_eq!(
            MediaKind::from_path(&dir.join("project.json")),
            Some(MediaKind::Web)
        );
        std::fs::write(dir.join("project.json"), r#"{"type": "scene"}"#).unwrap();
        std::fs::write(dir.join("scene.pkg"), b"").unwrap();
        assert_eq!(MediaKind::from_path(&dir), Some(MediaKind::Scene));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn end_to_end_over_local_socket() {
        // Real socket round trip in-process (unique name so a running lumend is unaffected).
        let name = format!("lumen-test-{}.sock", std::process::id());
        let listener = ListenerOptions::new()
            .name(name.clone().to_ns_name::<GenericNamespaced>().unwrap())
            .create_sync()
            .unwrap();
        let server = Server { listener };
        std::thread::spawn(move || {
            server.serve(|req| match req {
                Request::Pause => Response::Ok,
                _ => Response::Error {
                    message: "unexpected".into(),
                },
            });
        });
        let mut stream = ConnectOptions::new()
            .name(name.to_ns_name::<GenericNamespaced>().unwrap())
            .connect_sync()
            .unwrap();
        stream.write_all(&encode(&Request::Pause).unwrap()).unwrap();
        let mut reader = BufReader::new(stream);
        let resp: Response = decode(&read_line(&mut reader).unwrap()).unwrap();
        assert_eq!(resp, Response::Ok);
    }
}
