//! Serves the wallpaper's folder to its page (`diorama://<host>/<path>`), and
//! the files the user picked in its properties (`local::PREFIX`).
//! Workshop content is untrusted: paths stay inside the folder.

use std::borrow::Cow;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use wry::http::{Request, Response, StatusCode, header};

use crate::local::{LocalFiles, PREFIX};

/// Largest single response body (a byte range of a bigger file).
const MAX_BODY: u64 = 64 << 20;

/// Answers `request` from `root` (canonical) or the user's `local` files.
pub fn serve(
    root: &Path,
    local: &LocalFiles,
    request: &Request<Vec<u8>>,
) -> Response<Cow<'static, [u8]>> {
    let path = percent_decode(request.uri().path());
    let file = match path.strip_prefix(PREFIX) {
        Some(rest) => local.resolve(rest).or_else(|| {
            tracing::debug!("web wallpaper asked for {rest}, which no property picked");
            None
        }),
        None => resolve(root, &path),
    };
    let Some(file) = file else {
        return status(StatusCode::NOT_FOUND);
    };
    let Ok(mut f) = std::fs::File::open(&file) else {
        return status(StatusCode::NOT_FOUND);
    };
    let len = f.metadata().map_or(0, |m| m.len());
    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_range(v, len));
    let (start, end, partial) = match range {
        Some((s, e)) => (s, e, true),
        None => (0, len.saturating_sub(1), false),
    };
    let count = if len == 0 {
        0
    } else {
        (end - start + 1).min(MAX_BODY)
    };
    let mut body = vec![0; usize::try_from(count).unwrap_or(0)];
    if f.seek(SeekFrom::Start(start)).is_err() || f.read_exact(&mut body).is_err() {
        return status(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let mut response = Response::builder()
        .header(header::CONTENT_TYPE, mime(&file))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-cache");
    if partial {
        response = response.status(StatusCode::PARTIAL_CONTENT).header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{}/{len}", start + count.saturating_sub(1)),
        );
    }
    response
        .body(Cow::Owned(body))
        .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR))
}

fn status(code: StatusCode) -> Response<Cow<'static, [u8]>> {
    let mut r = Response::new(Cow::Borrowed(&[][..]));
    *r.status_mut() = code;
    r
}

/// The file `url_path` names inside `root`, or `None` if it leaves the
/// folder, is not a file, or does not exist. `root` must be canonical.
pub fn resolve(root: &Path, url_path: &str) -> Option<PathBuf> {
    let rel = Path::new(url_path.trim_start_matches('/'));
    // Only plain names: no `..`, roots, drive prefixes.
    if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let full = root.join(rel).canonicalize().ok()?;
    (full.starts_with(root) && full.is_file()).then_some(full)
}

/// `bytes=a-b` / `bytes=a-` / `bytes=-n` → inclusive (start, end).
fn parse_range(v: &str, len: u64) -> Option<(u64, u64)> {
    let spec = v.strip_prefix("bytes=")?.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    if len == 0 {
        return None;
    }
    let (start, end) = match (a.parse::<u64>().ok(), b.parse::<u64>().ok()) {
        (Some(s), Some(e)) => (s, e.min(len - 1)),
        (Some(s), None) => (s, len - 1),
        (None, Some(n)) => (len.saturating_sub(n), len - 1),
        (None, None) => return None,
    };
    (start <= end).then_some((start, end))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (
                b.get(i + 1).copied().and_then(hex),
                b.get(i + 2).copied().and_then(hex),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn mime(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_folder() {
        let dir = std::env::temp_dir().join("diorama-webhost-protocol-test");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/a b.txt"), b"hi").unwrap();
        let root = dir.canonicalize().unwrap();
        assert!(resolve(&root, "/sub/a b.txt").is_some());
        assert!(resolve(&root, &percent_decode("/sub/a%20b.txt")).is_some());
        assert!(resolve(&root, "/../secret").is_none());
        assert!(resolve(&root, "/sub/../sub/a b.txt").is_none());
        assert!(resolve(&root, "/C:/Windows/win.ini").is_none());
        assert!(resolve(&root, "/sub").is_none());
    }

    #[test]
    fn byte_ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=900-", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=0-5000", 1000), Some((0, 999)));
        assert_eq!(parse_range("bytes=5-1", 1000), None);
    }
}
