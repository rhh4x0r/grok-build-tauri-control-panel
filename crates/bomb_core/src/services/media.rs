//! Pictures and videos a thread shows, read in pieces by clients on another device (the phone).
//!
//! A client may only read a file the thread itself shows: inside its folder or project, among
//! the images saved for it, or named in its transcript. And only pictures and videos.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::Serialize;
use uuid::Uuid;

use crate::state::AppState;

/// The most one request returns; a client asks again from where it left off.
pub const MAX_CHUNK: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct MediaChunk {
    /// The whole file's size in bytes.
    pub size: u64,
    pub mime_type: String,
    /// Base64 of the bytes from the requested offset.
    pub data: String,
}

/// "image/png" or "video/mp4" for a picture or video path; `None` for anything else.
pub fn media_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        _ => return None,
    })
}

pub async fn read_media(state: &AppState, thread: String, path: String, offset: u64, length: Option<u64>) -> Result<MediaChunk, String> {
    let id = Uuid::parse_str(&thread).map_err(|e| e.to_string())?;
    let requested = PathBuf::from(&path);
    if !requested.is_absolute() {
        return Err("Ask for a picture or video by its full path.".into());
    }
    let mime_type = media_type(&requested).ok_or("Only pictures and videos can be opened from here.")?;
    let file = std::fs::canonicalize(&requested).map_err(|_| "That file isn't on this machine anymore.".to_string())?;
    if media_type(&file).is_none() {
        return Err("Only pictures and videos can be opened from here.".into());
    }
    if !belongs_to_thread(state, id, &path, &file)? {
        return Err("That file isn't part of this thread.".into());
    }
    let length = length.unwrap_or(MAX_CHUNK).min(MAX_CHUNK);
    tokio::task::spawn_blocking(move || {
        let mut f = std::fs::File::open(&file).map_err(|e| e.to_string())?;
        let size = f.metadata().map_err(|e| e.to_string())?.len();
        let mut bytes = Vec::with_capacity(length.min(size.saturating_sub(offset)) as usize);
        f.seek(SeekFrom::Start(offset.min(size))).map_err(|e| e.to_string())?;
        f.take(length).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        Ok(MediaChunk { size, mime_type: mime_type.to_string(), data: base64::engine::general_purpose::STANDARD.encode(bytes) })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Inside the thread's folder or project, among its saved images, or named in its transcript.
fn belongs_to_thread(state: &AppState, id: Uuid, asked: &str, file: &Path) -> Result<bool, String> {
    let record = state.persistence.get_session(id).map_err(|e| e.to_string())?;
    let mut roots = vec![PathBuf::from(&record.cwd)];
    if let Some(root) = serde_json::from_str::<serde_json::Value>(&record.metadata_json).ok().and_then(|v| {
        v.pointer("/metadata/projectRoot").or_else(|| v.pointer("/metadata/project_root")).and_then(|r| r.as_str()).map(PathBuf::from)
    }) {
        roots.push(root);
    }
    if let Some(home) = grok_config::paths::bomb_home() {
        roots.push(home.join("sessions").join("images").join(id.to_string()));
    }
    let inside = roots.iter().filter_map(|r| std::fs::canonicalize(r).ok()).any(|r| file.starts_with(&r));
    if inside {
        return Ok(true);
    }
    let shown = file.to_string_lossy();
    let rows = state.persistence.transcripts(id).map_err(|e| e.to_string())?;
    Ok(rows.iter().any(|row| row.payload.contains(asked) || row.payload.contains(shown.as_ref())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_pictures_and_videos_have_a_media_type() {
        assert_eq!(media_type(Path::new("/a/b.PNG")), Some("image/png"));
        assert_eq!(media_type(Path::new("/a/clip.mov")), Some("video/quicktime"));
        assert_eq!(media_type(Path::new("/a/.env")), None);
        assert_eq!(media_type(Path::new("/a/notes.txt")), None);
        assert_eq!(media_type(Path::new("/a/noext")), None);
    }
}
