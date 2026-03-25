use std::path::Path;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tracing::{debug, info, warn};

use crate::error::SyncError;

/// Buffer size for streaming response body to disk (256 KiB).
const WRITE_BUFFER_SIZE: usize = 256 * 1024;

// ── Download Progress ────────────────────────────────────────────────────────

/// Current state of a download, used for progress reporting and resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
}

// ── Download ─────────────────────────────────────────────────────────────────

/// Download a file from a presigned URL, verify its BLAKE3 hash, and
/// atomically rename it into place.
///
/// 1. Stream response body to `<dest_path>.sessionsync-tmp`.
/// 2. If the temp file already exists (partial download), resume using a
///    `Range` header.
/// 3. On completion, compute BLAKE3 hash of the full temp file.
/// 4. If hash matches `expected_hash`, atomically rename to `dest_path`.
/// 5. If hash mismatches, delete temp file and return `HashMismatch` error.
///
/// The `progress_cb` receives `(bytes_downloaded, total_bytes)`.
pub async fn download_file<F>(
    client: &Client,
    presigned_url: &str,
    dest_path: &Path,
    expected_hash: &str,
    progress_cb: F,
) -> Result<(), SyncError>
where
    F: Fn(u64, u64) + Send + Sync + 'static,
{
    let temp_path = dest_path.with_extension(
        dest_path
            .extension()
            .map(|ext| format!("{}.sessionsync-tmp", ext.to_string_lossy()))
            .unwrap_or_else(|| "sessionsync-tmp".to_string()),
    );

    // Ensure the parent directory exists.
    if let Some(parent) = dest_path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| SyncError::FileSystem {
            path: parent.display().to_string(),
            source: e,
        })?;
    }

    // Check for an existing partial download.
    let existing_bytes = match tokio::fs::metadata(&temp_path).await {
        Ok(meta) => meta.len(),
        Err(_) => 0,
    };

    // Build request, optionally with a Range header for resume.
    let mut request = client.get(presigned_url);
    if existing_bytes > 0 {
        info!(
            "Resuming download of {} from byte {}",
            dest_path.display(),
            existing_bytes
        );
        request = request.header("Range", format!("bytes={}-", existing_bytes));
    }

    let response = request.send().await?;

    if !response.status().is_success() && response.status().as_u16() != 206 {
        let status = response.status();
        let body_text = response.text().await.unwrap_or_default();
        return Err(SyncError::R2Download {
            key: dest_path.display().to_string(),
            source: format!("HTTP {status}: {body_text}").into(),
        });
    }

    // Determine total file size from Content-Range or Content-Length.
    let total_bytes = parse_total_bytes(&response, existing_bytes);
    let is_resume = response.status().as_u16() == 206;

    info!(
        "Downloading {} ({} bytes total, resume={})",
        dest_path.display(),
        total_bytes,
        is_resume
    );

    // Open file for writing (append if resuming, create otherwise).
    let file = if is_resume {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(&temp_path)
            .await
            .map_err(|e| SyncError::FileSystem {
                path: temp_path.display().to_string(),
                source: e,
            })?
    } else {
        tokio::fs::File::create(&temp_path).await.map_err(|e| SyncError::FileSystem {
            path: temp_path.display().to_string(),
            source: e,
        })?
    };

    let mut writer = tokio::io::BufWriter::with_capacity(WRITE_BUFFER_SIZE, file);
    let mut bytes_downloaded = existing_bytes;

    // Stream the response body to disk.
    let mut stream = response.bytes_stream();
    use futures_util::StreamExt;

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;
        writer.write_all(&chunk).await.map_err(|e| SyncError::FileSystem {
            path: temp_path.display().to_string(),
            source: e,
        })?;
        bytes_downloaded += chunk.len() as u64;
        progress_cb(bytes_downloaded, total_bytes);
    }

    writer.flush().await.map_err(|e| SyncError::FileSystem {
        path: temp_path.display().to_string(),
        source: e,
    })?;
    // Explicitly drop writer to close the file before hashing.
    drop(writer);

    debug!(
        "Download stream complete for {}, verifying hash",
        dest_path.display()
    );

    // Verify BLAKE3 hash of the complete file.
    let actual_hash = hash_file(&temp_path).await?;

    if actual_hash != expected_hash {
        warn!(
            "Hash mismatch for {}: expected {}, got {}",
            dest_path.display(),
            expected_hash,
            actual_hash
        );
        // Clean up the bad temp file.
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(SyncError::HashMismatch {
            path: dest_path.display().to_string(),
            expected: expected_hash.to_string(),
            actual: actual_hash,
        });
    }

    // Atomic rename from temp to final path.
    tokio::fs::rename(&temp_path, dest_path).await.map_err(|e| SyncError::FileSystem {
        path: dest_path.display().to_string(),
        source: e,
    })?;

    progress_cb(total_bytes, total_bytes);
    info!("Download verified and complete: {}", dest_path.display());
    Ok(())
}

// ── BLAKE3 Hashing ───────────────────────────────────────────────────────────

/// Compute the BLAKE3 hash of a file. Uses memory-mapped I/O for large files
/// and falls back to buffered reads otherwise.
async fn hash_file(path: &Path) -> Result<String, SyncError> {
    let path = path.to_path_buf();

    // Run the CPU-intensive hash on a blocking thread.
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&path).map_err(|e| SyncError::FileSystem {
            path: path.display().to_string(),
            source: e,
        })?;
        let metadata = file.metadata().map_err(|e| SyncError::FileSystem {
            path: path.display().to_string(),
            source: e,
        })?;

        let hash = if metadata.len() > 128 * 1024 * 1024 {
            // For files > 128 MiB, use mmap for performance.
            let mmap = unsafe {
                memmap2::Mmap::map(&file).map_err(|e| SyncError::FileSystem {
                    path: path.display().to_string(),
                    source: e,
                })?
            };
            blake3::hash(&mmap)
        } else {
            // For smaller files, use buffered reading.
            let mut hasher = blake3::Hasher::new();
            let mut reader = std::io::BufReader::with_capacity(1024 * 1024, file);
            std::io::copy(&mut reader, &mut hasher).map_err(|e| SyncError::FileSystem {
                path: path.display().to_string(),
                source: e,
            })?;
            hasher.finalize()
        };

        Ok(hash.to_hex().to_string())
    })
    .await
    .map_err(|e| SyncError::Other(format!("Hash task panicked: {e}")))?
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Parse total file size from response headers. Handles both full responses
/// (Content-Length) and partial responses (Content-Range).
fn parse_total_bytes(response: &reqwest::Response, existing_bytes: u64) -> u64 {
    // Try Content-Range header first (for 206 responses):
    // "bytes 1000-9999/10000" -> total is 10000
    if let Some(range) = response.headers().get("content-range") {
        if let Ok(range_str) = range.to_str() {
            if let Some(slash_pos) = range_str.rfind('/') {
                if let Ok(total) = range_str[slash_pos + 1..].parse::<u64>() {
                    return total;
                }
            }
        }
    }

    // Fall back to Content-Length + existing bytes.
    response
        .content_length()
        .map(|cl| cl + existing_bytes)
        .unwrap_or(0)
}
