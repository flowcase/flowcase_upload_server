use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use tracing::warn;

#[derive(Clone)]
pub struct UploadDir(pub Arc<PathBuf>);

impl UploadDir {
    pub fn new(p: PathBuf) -> Self {
        Self(Arc::new(p))
    }
}

struct ChunkInfo {
    file_data: Bytes,
    file_name: String,
    chunk_index: u64,
    chunk_byte_offset: u64,
    total_file_size: u64,
    total_chunk_count: u64,
}

pub async fn handle_upload(State(dir): State<UploadDir>, multipart: Multipart) -> Response {
    let info = match parse_multipart(multipart).await {
        Ok(i) => i,
        Err(resp) => return resp,
    };

    let sanitized = sanitize_filename(&info.file_name);
    if sanitized.is_empty() {
        return bad_request("Invalid filename");
    }

    let final_path = dir.0.join(&sanitized);
    let upload_path = dir.0.join(format!(".{sanitized}.uploading"));

    if info.chunk_index == 0 {
        if final_path.exists() {
            return bad_request("File already exists");
        }
        if upload_path.exists() {
            if let Err(err) = std::fs::remove_file(&upload_path) {
                warn!(?err, path=%upload_path.display(), "stale .uploading remove failed");
                return internal("Couldn't write the file to disk");
            }
        }
        match free_space(&dir.0) {
            Ok(space) if space < info.total_file_size => {
                return bad_request("No Space available");
            }
            Ok(_) => {}
            Err(err) => {
                // statvfs failure is logged but doesn't block — the
                // legacy Python would have crashed with the exception;
                // fall through and let the actual write surface ENOSPC.
                warn!(?err, "statvfs failed; skipping space pre-check");
            }
        }
    }

    // Append-mode write. The kernel ignores the file position on writes
    // when O_APPEND is set, so the seek is a parity-with-Python no-op
    // for in-order chunks (Dropzone always sends in order).
    let write_result = (|| -> std::io::Result<()> {
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&upload_path)?;
        f.seek(SeekFrom::Start(info.chunk_byte_offset))?;
        f.write_all(&info.file_data)?;
        Ok(())
    })();
    if let Err(err) = write_result {
        warn!(?err, path=%upload_path.display(), "chunk write failed");
        return internal("Couldn't write the file to disk");
    }

    if info.chunk_index + 1 == info.total_chunk_count {
        let actual_size = match std::fs::metadata(&upload_path) {
            Ok(m) => m.len(),
            Err(err) => {
                warn!(?err, "stat .uploading failed");
                return internal("Couldn't write the file to disk");
            }
        };
        if actual_size != info.total_file_size {
            let _ = std::fs::remove_file(&upload_path);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Size mismatch").into_response();
        }
        if let Err(err) = std::fs::rename(&upload_path, &final_path) {
            warn!(?err, "final rename failed");
            return internal("Couldn't write the file to disk");
        }
    }

    (StatusCode::OK, "uploaded Chunk").into_response()
}

async fn parse_multipart(mut mp: Multipart) -> Result<ChunkInfo, Response> {
    let mut file_data: Option<Bytes> = None;
    let mut file_name: Option<String> = None;
    let mut chunk_index: Option<u64> = None;
    let mut chunk_byte_offset: Option<u64> = None;
    let mut total_file_size: Option<u64> = None;
    let mut total_chunk_count: Option<u64> = None;

    loop {
        let field_opt = match mp.next_field().await {
            Ok(f) => f,
            Err(err) => {
                warn!(?err, "multipart parse error");
                return Err(bad_request("Invalid multipart"));
            }
        };
        let Some(field) = field_opt else { break };
        let name = field.name().map(str::to_string);
        match name.as_deref() {
            Some("file") => {
                file_name = field.file_name().map(str::to_string);
                let bytes = field.bytes().await.map_err(|err| {
                    warn!(?err, "reading file bytes failed");
                    internal("Couldn't write the file to disk")
                })?;
                file_data = Some(bytes);
            }
            Some("dzchunkindex") => chunk_index = parse_field(field).await,
            Some("dzchunkbyteoffset") => chunk_byte_offset = parse_field(field).await,
            Some("dztotalfilesize") => total_file_size = parse_field(field).await,
            Some("dztotalchunkcount") => total_chunk_count = parse_field(field).await,
            _ => {} // ignore unknown fields
        }
    }

    Ok(ChunkInfo {
        file_data: file_data.ok_or_else(|| bad_request("Missing file"))?,
        file_name: file_name.ok_or_else(|| bad_request("Missing filename"))?,
        chunk_index: chunk_index.ok_or_else(|| bad_request("Missing dzchunkindex"))?,
        chunk_byte_offset: chunk_byte_offset
            .ok_or_else(|| bad_request("Missing dzchunkbyteoffset"))?,
        total_file_size: total_file_size.ok_or_else(|| bad_request("Missing dztotalfilesize"))?,
        total_chunk_count: total_chunk_count
            .ok_or_else(|| bad_request("Missing dztotalchunkcount"))?,
    })
}

async fn parse_field(field: axum::extract::multipart::Field<'_>) -> Option<u64> {
    field.text().await.ok().and_then(|s| s.parse::<u64>().ok())
}

/// Mirror legacy escapeFilename: keep alphanumeric + space/dot/underscore/
/// hyphen, then rstrip whitespace.
fn sanitize_filename(name: &str) -> String {
    let kept: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-'))
        .collect();
    kept.trim_end().to_string()
}

fn free_space(dir: &Path) -> Result<u64, nix::Error> {
    let s = nix::sys::statvfs::statvfs(dir)?;
    Ok((s.fragment_size() as u64).saturating_mul(s.blocks_available() as u64))
}

fn bad_request(msg: &'static str) -> Response {
    (StatusCode::BAD_REQUEST, msg).into_response()
}

fn internal(msg: &'static str) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use axum::routing::post;
    use axum::Router;
    use tower::ServiceExt;

    const BOUNDARY: &str = "BOUNDARY";

    fn build_multipart(
        chunk: &[u8],
        filename: &str,
        index: u64,
        offset: u64,
        total_size: u64,
        total_chunks: u64,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        let push_field = |out: &mut Vec<u8>, name: &str, value: &str| {
            out.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
            out.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
            );
            out.extend_from_slice(value.as_bytes());
            out.extend_from_slice(b"\r\n");
        };
        push_field(&mut out, "dzchunkindex", &index.to_string());
        push_field(&mut out, "dzchunkbyteoffset", &offset.to_string());
        push_field(&mut out, "dztotalfilesize", &total_size.to_string());
        push_field(&mut out, "dztotalchunkcount", &total_chunks.to_string());

        out.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        out.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
                 Content-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
        out
    }

    fn app(dir: PathBuf) -> Router {
        Router::new()
            .route("/upload", post(handle_upload))
            .with_state(UploadDir::new(dir))
    }

    async fn upload(
        app: &Router,
        chunk: &[u8],
        filename: &str,
        index: u64,
        offset: u64,
        total_size: u64,
        total_chunks: u64,
    ) -> Response {
        let body = build_multipart(chunk, filename, index, offset, total_size, total_chunks);
        let req = Request::builder()
            .method("POST")
            .uri("/upload")
            .header(
                "Content-Type",
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .body(Body::from(body))
            .unwrap();
        app.clone().oneshot(req).await.unwrap()
    }

    async fn body_string(resp: Response) -> (StatusCode, String) {
        let status = resp.status();
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn three_chunks_assemble_into_final_file() {
        let tmp = tempfile::tempdir().unwrap();
        let app = app(tmp.path().to_path_buf());

        let payload: &[u8] = b"abcdefghij0123456789ABCDEFGHIJ"; // 30 bytes
        assert_eq!(payload.len(), 30);
        let chunk_size = 10u64;
        let total_size = payload.len() as u64;
        let total_chunks = 3u64;

        for i in 0..total_chunks {
            let start = (i * chunk_size) as usize;
            let end = start + chunk_size as usize;
            let resp = upload(
                &app,
                &payload[start..end],
                "hello.txt",
                i,
                i * chunk_size,
                total_size,
                total_chunks,
            )
            .await;
            assert_eq!(resp.status(), StatusCode::OK, "chunk {i} should be 200");
        }

        let final_path = tmp.path().join("hello.txt");
        assert!(final_path.exists(), "final file should exist");
        assert_eq!(std::fs::read(&final_path).unwrap(), payload);
        assert!(
            !tmp.path().join(".hello.txt.uploading").exists(),
            ".uploading should be gone after rename"
        );
    }

    #[tokio::test]
    async fn first_chunk_against_existing_final_returns_409_style_400() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("dup.bin"), b"existing").unwrap();
        let app = app(tmp.path().to_path_buf());

        let resp = upload(&app, b"new", "dup.bin", 0, 0, 3, 1).await;
        let (status, body) = body_string(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, "File already exists");
    }

    #[tokio::test]
    async fn size_mismatch_on_last_chunk_returns_500() {
        let tmp = tempfile::tempdir().unwrap();
        let app = app(tmp.path().to_path_buf());

        // Claim a 10-byte file but only send 5 bytes as the only chunk.
        let resp = upload(&app, b"hello", "short.bin", 0, 0, 10, 1).await;
        let (status, body) = body_string(resp).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body, "Size mismatch");
        assert!(
            !tmp.path().join(".short.bin.uploading").exists(),
            ".uploading should be cleaned up on size mismatch"
        );
    }

    #[tokio::test]
    async fn filename_is_sanitized() {
        assert_eq!(sanitize_filename("hello world.txt"), "hello world.txt");
        assert_eq!(sanitize_filename("evil/../path.bin"), "evil..path.bin");
        assert_eq!(sanitize_filename("trailing  "), "trailing");
        assert_eq!(sanitize_filename("emoji🚀name.txt"), "emojiname.txt");
        assert_eq!(sanitize_filename("a_b-c.d e"), "a_b-c.d e");
    }

    #[tokio::test]
    async fn missing_file_field_is_400() {
        let tmp = tempfile::tempdir().unwrap();
        let app = app(tmp.path().to_path_buf());

        // Body with form fields but no `file`.
        let mut body = Vec::new();
        let push = |out: &mut Vec<u8>, n: &str, v: &str| {
            out.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
            out.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{n}\"\r\n\r\n").as_bytes(),
            );
            out.extend_from_slice(v.as_bytes());
            out.extend_from_slice(b"\r\n");
        };
        push(&mut body, "dzchunkindex", "0");
        push(&mut body, "dzchunkbyteoffset", "0");
        push(&mut body, "dztotalfilesize", "5");
        push(&mut body, "dztotalchunkcount", "1");
        body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());

        let req = Request::builder()
            .method("POST")
            .uri("/upload")
            .header(
                "Content-Type",
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .body(Body::from(body))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let (status, body) = body_string(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, "Missing file");
    }
}
