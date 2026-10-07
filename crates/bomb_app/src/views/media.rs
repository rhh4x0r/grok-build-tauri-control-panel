//! Videos an agent links in a reply (`![title](/path/clip.mp4)`): a card in the chat,
//! played in the right panel's Preview.
//!
//! The preview's web view gets files through a private `bomb-media://` scheme that
//! serves byte ranges (players need them to seek), and only for videos the person has
//! clicked in the chat, so a page open in Preview can't read anything else on disk.

use std::borrow::Cow;
use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use wry::http::{header, Request, Response, StatusCode};

pub const SCHEME: &str = "bomb-media";
const VIDEO_EXTS: &[&str] = &["mp4", "m4v", "mov", "webm"];

fn allowed() -> &'static Mutex<HashSet<PathBuf>> {
    static ALLOWED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    ALLOWED.get_or_init(Default::default)
}

pub fn is_video_path(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    VIDEO_EXTS.iter().any(|e| lower.ends_with(&format!(".{e}")))
}

/// The file's own address, after allowing it to be served.
fn file_url(path: &Path) -> String {
    allowed().lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_path_buf());
    format!("{SCHEME}://localhost{}", encode_path(&path.to_string_lossy()))
}

/// A small page that plays `path` scaled to fit, starting at `from` seconds.
pub fn player_url(path: &Path, from: f64, autoplay: bool) -> String {
    let src = encode_path(&file_url(path));
    format!("{SCHEME}://localhost/__player?src={src}&t={from:.2}&autoplay={}", autoplay as u8)
}

fn player_page(query: &str) -> String {
    let get = |key: &str| {
        query.split('&').find_map(|kv| kv.strip_prefix(&format!("{key}="))).map(decode).unwrap_or_default()
    };
    let attr = |s: String| s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
    let src = attr(get("src"));
    let from: f64 = get("t").parse().unwrap_or(0.0);
    let autoplay = if get("autoplay") == "1" { " autoplay" } else { "" };
    format!(
        "<!doctype html><meta charset=utf-8><style>html,body{{margin:0;height:100%;background:#000;overflow:hidden}}\
         video{{width:100%;height:100%;object-fit:contain;display:block}}</style>\
         <video src=\"{src}#t={from:.2}\" controls playsinline{autoplay}></video>"
    )
}

/// A video linked in a reply: what the card is called and the file.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkedVideo {
    pub title: String,
    pub path: PathBuf,
}

/// Videos in `text`: Markdown targets (`![t](p)`, `[t](p)`) and bare paths, resolved
/// against the thread folder; only files that exist count.
pub fn linked_videos(text: &str, cwd: &Path) -> Vec<LinkedVideo> {
    let mut out: Vec<LinkedVideo> = Vec::new();
    let mut add = |title: Option<&str>, target: &str| {
        let target = decode(target.trim().trim_start_matches("file://").trim_matches(|c| matches!(c, '<' | '>' | '`' | '"')));
        if !is_video_path(&target) || target.starts_with("http") {
            return;
        }
        let p = Path::new(&target);
        let path = if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) };
        if !path.is_file() || out.iter().any(|v| v.path == path) {
            return;
        }
        let title = title
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(String::from)
            .or_else(|| path.file_stem().map(|s| s.to_string_lossy().replace('_', " ")))
            .unwrap_or_default();
        out.push(LinkedVideo { title, path });
    };
    let mut rest = text;
    while let Some(i) = rest.find("](") {
        let title_start = rest[..i].rfind('[').map(|s| s + 1).unwrap_or(i);
        let title = &rest[title_start..i];
        let after = &rest[i + 2..];
        let Some(j) = after.find(')') else { break };
        add(Some(title), &after[..j]);
        rest = &after[j + 1..];
    }
    for token in text.split_whitespace() {
        if token.starts_with('/') || token.starts_with("file://") {
            add(None, token.trim_end_matches(['.', ',', ')', ';']));
        }
    }
    out
}

/// A cached Quick Look thumbnail, made in the background the first time it's asked for.
/// `None` until it exists.
pub fn thumbnail(video: &Path) -> Option<PathBuf> {
    let dir = thumbs_dir()?.join(format!("{:016x}", hash(video)));
    let name = format!("{}.png", video.file_name()?.to_string_lossy());
    let png = dir.join(name);
    if png.is_file() {
        return Some(png);
    }
    static PENDING: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let pending = PENDING.get_or_init(Default::default);
    if pending.lock().unwrap_or_else(|e| e.into_inner()).insert(video.to_path_buf()) {
        let video = video.to_path_buf();
        std::thread::spawn(move || {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::process::Command::new("qlmanage")
                .args(["-t", "-s", "480", "-o"])
                .arg(&dir)
                .arg(&video)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        });
    }
    None
}

fn thumbs_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".grok/control-panel/thumbnails"))
}

fn hash(p: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.hash(&mut h);
    std::fs::metadata(p).and_then(|m| m.modified()).ok().hash(&mut h);
    h.finish()
}

/// Serve an allowed video, honouring `Range` so the player can seek.
pub fn serve(request: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let fail = |status: StatusCode| Response::builder().status(status).body(Cow::Borrowed(&[][..])).unwrap();
    if request.uri().path() == "/__player" {
        let page = player_page(request.uri().query().unwrap_or(""));
        return Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Cow::Owned(page.into_bytes()))
            .unwrap_or_else(|_| fail(StatusCode::INTERNAL_SERVER_ERROR));
    }
    let path = PathBuf::from(decode(request.uri().path()));
    let permitted = allowed().lock().unwrap_or_else(|e| e.into_inner()).contains(&path);
    if !permitted || !is_video_path(&path.to_string_lossy()) {
        return fail(StatusCode::FORBIDDEN);
    }
    let Ok(mut file) = std::fs::File::open(&path) else { return fail(StatusCode::NOT_FOUND) };
    let Ok(len) = file.metadata().map(|m| m.len()) else { return fail(StatusCode::NOT_FOUND) };
    let mime = match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        _ => "video/mp4",
    };
    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|r| parse_range(r, len));
    // Without a range, send the start: players ask for the rest in ranges.
    let (start, end, partial) = match range {
        Some((s, e)) => (s, e, true),
        None => (0, len.saturating_sub(1).min(4 * 1024 * 1024), len > 4 * 1024 * 1024),
    };
    // Cap one response so a seek never reads a whole large file into memory.
    let end = end.min(start + 8 * 1024 * 1024 - 1);
    let mut buf = vec![0u8; (end + 1 - start) as usize];
    if file.seek(SeekFrom::Start(start)).and_then(|_| file.read_exact(&mut buf)).is_err() {
        return fail(StatusCode::RANGE_NOT_SATISFIABLE);
    }
    let mut response = Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, buf.len().to_string());
    if partial {
        response = response
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"));
    }
    response.body(Cow::Owned(buf)).unwrap_or_else(|_| fail(StatusCode::INTERNAL_SERVER_ERROR))
}

/// `bytes=a-b`, `bytes=a-` or `bytes=-n` → inclusive (start, end) within `len`.
fn parse_range(header: &str, len: u64) -> Option<(u64, u64)> {
    let spec = header.strip_prefix("bytes=")?.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    let last = len.checked_sub(1)?;
    let (start, end) = match (a.trim(), b.trim()) {
        ("", n) => (len.saturating_sub(n.parse().ok()?), last),
        (s, "") => (s.parse().ok()?, last),
        (s, e) => (s.parse().ok()?, e.parse::<u64>().ok()?.min(last)),
    };
    (start <= end).then_some((start, end))
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn encode_path(p: &str) -> String {
    p.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_linked_videos_with_titles() {
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("My Clip_720.mp4");
        std::fs::write(&clip, b"x").unwrap();
        let encoded = encode_path(&clip.to_string_lossy());
        let text = format!("**Done — 27s**\n![You Can Say No]({encoded})\n![img](/nope.png) and {} again", clip.display());
        let found = linked_videos(&text, dir.path());
        assert_eq!(found, vec![LinkedVideo { title: "You Can Say No".into(), path: clip }]);
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=900-", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=5-2", 1000), None);
    }

    #[test]
    fn serves_only_clicked_videos() {
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("a b.mp4");
        std::fs::write(&clip, b"0123456789").unwrap();
        let url = format!("{SCHEME}://localhost{}", encode_path(&clip.to_string_lossy()));
        let get = |range: Option<&str>| {
            let mut r = Request::builder().uri(url.as_str());
            if let Some(range) = range { r = r.header(header::RANGE, range); }
            serve(r.body(Vec::new()).unwrap())
        };
        assert_eq!(get(None).status(), StatusCode::FORBIDDEN);
        assert_eq!(file_url(&clip), url);
        let page = serve(Request::builder().uri(player_url(&clip, 3.5, true)).body(Vec::new()).unwrap());
        let html = String::from_utf8(page.body().to_vec()).unwrap();
        assert!(html.contains(&format!("src=\"{url}#t=3.50\"")) && html.contains("autoplay"), "{html}");
        let whole = get(None);
        assert_eq!((whole.status(), whole.body().as_ref()), (StatusCode::OK, &b"0123456789"[..]));
        let part = get(Some("bytes=2-4"));
        assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(part.body().as_ref(), b"234");
        assert_eq!(part.headers()[header::CONTENT_RANGE], "bytes 2-4/10");
    }
}

// ── Inline player ───────────────────────────────────────────────────────
// GPUI has no video element, so a card plays through a small native web view. A native
// view can't be clipped to the scrolling chat: it is shown only while its whole card is
// inside the visible area, and `PlayerWatch` (after the chat list) hides and pauses it
// when the card wasn't drawn at all (another thread, scrolled far away).

pub use inline::{aspect, expand_inline, play_inline, player_watch, stop_inline, video_slot};

mod inline {
use super::{player_url, serve, SCHEME};
use gpui_kit::*;
use std::cell::Cell;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub struct InlinePlayer {
    pub path: PathBuf,
    view: Entity<gpui_wry::WebView>,
    /// Set when the card's slot is laid out this frame.
    drawn: Rc<Cell<bool>>,
}

#[derive(Default)]
pub struct InlinePlayerSlot(pub Option<InlinePlayer>);
impl Global for InlinePlayerSlot {}

fn slot(cx: &App) -> Option<&InlinePlayer> {
    cx.try_global::<InlinePlayerSlot>().and_then(|s| s.0.as_ref())
}

/// Play `path` in its card, replacing any other inline video.
pub fn play_inline(path: &Path, window: &mut Window, cx: &mut App) {
    let url = player_url(path, 0.0, true);
    let built = wry::WebViewBuilder::new()
        .with_custom_protocol(SCHEME.into(), |_, request| serve(request))
        .with_url(&url)
        .build_as_child(window);
    match built {
        Ok(raw) => {
            let view = cx.new(|cx| gpui_wry::WebView::new(raw, window, cx));
            cx.set_global(InlinePlayerSlot(Some(InlinePlayer { path: path.to_path_buf(), view, drawn: Rc::new(Cell::new(false)) })));
        }
        Err(e) => tracing::warn!(error = %e, "inline video: could not create a player"),
    }
    window.refresh();
}

/// Stop the inline video (dropping the view hides it).
pub fn stop_inline(cx: &mut App) {
    if cx.has_global::<InlinePlayerSlot>() {
        cx.set_global(InlinePlayerSlot(None));
    }
}

/// Move the inline video to the Preview panel, carrying on from the same moment.
pub fn expand_inline(cx: &mut App) {
    let Some(player) = slot(cx) else { return };
    let path = player.path.clone();
    let raw = player.view.read(cx).handle();
    let (tx, rx) = futures::channel::oneshot::channel::<f64>();
    let tx = std::sync::Mutex::new(Some(tx));
    let asked = raw.raw().evaluate_script_with_callback("(document.querySelector('video')||{}).currentTime||0", move |answer| {
        if let Some(tx) = tx.lock().ok().and_then(|mut t| t.take()) {
            let _ = tx.send(answer.trim_matches('"').parse().unwrap_or(0.0));
        }
    });
    let ask_ok = asked.is_ok();
    cx.spawn(async move |cx| {
        let from = if ask_ok { rx.await.unwrap_or(0.0) } else { 0.0 };
        cx.update(|cx| {
            stop_inline(cx);
            if let Some(model) = cx.try_global::<crate::models::app::AppModelHandle>().map(|h| h.0.clone()) {
                model.update(cx, |m, cx| { m.browse_request = Some(player_url(&path, from, true)); cx.notify(); });
            }
        });
    })
    .detach();
}

/// Where the inline video sits inside its card.
pub fn video_slot(path: &Path, cx: &App) -> Option<VideoSlot> {
    let player = slot(cx).filter(|p| p.path == path)?;
    Some(VideoSlot { raw: player.view.read(cx).handle(), drawn: player.drawn.clone() })
}

pub struct VideoSlot {
    raw: gpui_wry::WebViewHandle,
    drawn: Rc<Cell<bool>>,
}

impl IntoElement for VideoSlot {
    type Element = Self;
    fn into_element(self) -> Self { self }
}

impl Element for VideoSlot {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> { None }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> { None }
    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let style = Style { size: Size::full(), ..Default::default() };
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, _: &mut App) {
        self.drawn.set(true);
        let visible = window.content_mask().bounds;
        let whole = visible.intersect(&bounds) == bounds;
        let raw = self.raw.raw();
        if whole {
            let _ = raw.set_bounds(wry::Rect {
                size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(bounds.size.width.into(), bounds.size.height.into())),
                position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(bounds.origin.x.into(), bounds.origin.y.into())),
            });
        }
        let _ = raw.set_visible(whole);
    }
    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut (), _: &mut (), _: &mut Window, _: &mut App) {}
}

/// Placed after the chat list: hides and pauses the inline video when its card wasn't drawn.
pub fn player_watch(cx: &App) -> Option<impl IntoElement> {
    let player = slot(cx)?;
    let (raw, drawn) = (player.view.read(cx).handle(), player.drawn.clone());
    Some(
        canvas(
            move |_, _, _| {
                if !drawn.replace(false) {
                    let _ = raw.raw().set_visible(false);
                    let _ = raw.raw().evaluate_script("(document.querySelector('video')||{pause(){}}).pause()");
                }
            },
            |_, _, _, _| {},
        )
        .size_0(),
    )
}

/// Width ÷ height from a thumbnail PNG's header, for sizing a card like the video.
pub fn aspect(png: &Path) -> Option<f32> {
    let mut head = [0u8; 24];
    std::fs::File::open(png).ok()?.read_exact(&mut head).ok()?;
    let w = u32::from_be_bytes(head[16..20].try_into().ok()?) as f32;
    let h = u32::from_be_bytes(head[20..24].try_into().ok()?) as f32;
    (w > 0.0 && h > 0.0).then_some(w / h)
}
}
