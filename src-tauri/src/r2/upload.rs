use std::path::Path;
use std::sync::Arc;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, BufReader};
use tokio::sync::Semaphore;
use tracing::{debug, info};

use crate::error::SyncError;

/// 100 MiB threshold: files below this use a single PUT; files at or above
/// use multipart upload.
pub const MULTIPART_THRESHOLD: u64 = 100 * 1024 * 1024;

/// 100 MiB part size for multipart uploads.
pub const PART_SIZE: u64 = 100 * 1024 * 1024;

/// Maximum number of concurrent part uploads within a single multipart upload.
const MAX_CONCURRENT_PARTS: usize = 2;

/// Buffer size for reading file chunks (256 KiB).
const READ_BUFFER_SIZE: usize = 256 * 1024;

// ── Upload Progress ──────────────────────────────────────────────────────────

/// Tracks multipart upload progress for crash recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadProgress {
    pub parts_completed: Vec<u32>,
    pub total_parts: u32,
}

// ── Multipart URLs ───────────────────────────────────────────────────────────

/// Presigned URLs returned by Convex for a multipart upload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultipartUrls {
    /// URL to initiate the multipart upload (POST).
    pub create_url: String,
    /// One presigned PUT URL per part, in order.
    pub part_urls: Vec<String>,
    /// URL to complete the multipart upload (POST).
    pub complete_url: String,
}

// ── Single-Part Upload ───────────────────────────────────────────────────────

/// Upload a file smaller than 100 MiB using a single PUT request to a
/// presigned URL. Streams the file from disk to avoid loading it entirely
/// into memory.
///
/// The `progress_cb` is called periodically with `(bytes_uploaded, total_bytes)`.
pub async fn upload_file<F>(
    client: &Client,
    presigned_url: &str,
    file_path: &Path,
    progress_cb: F,
) -> Result<(), SyncError>
where
    F: Fn(u64, u64) + Send + Sync + 'static,
{
    let metadata = tokio::fs::metadata(file_path).await.map_err(|e| SyncError::FileSystem {
        path: file_path.display().to_string(),
        source: e,
    })?;
    let total_bytes = metadata.len();

    info!(
        "Uploading {} ({} bytes) via single PUT",
        file_path.display(),
        total_bytes
    );

    // Read the file into a body. For files < 100 MiB this is acceptable;
    // reqwest needs either a bytes body or a stream. We stream via chunks.
    let file = tokio::fs::File::open(file_path).await.map_err(|e| SyncError::FileSystem {
        path: file_path.display().to_string(),
        source: e,
    })?;

    let mut reader = BufReader::with_capacity(READ_BUFFER_SIZE, file);
    let mut body = Vec::with_capacity(total_bytes as usize);
    let mut bytes_read: u64 = 0;
    let mut buf = vec![0u8; READ_BUFFER_SIZE];

    loop {
        let n = reader.read(&mut buf).await.map_err(|e| SyncError::FileSystem {
            path: file_path.display().to_string(),
            source: e,
        })?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
        bytes_read += n as u64;
        progress_cb(bytes_read, total_bytes);
    }

    let response = client
        .put(presigned_url)
        .header("content-length", total_bytes)
        .body(body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body_text = response.text().await.unwrap_or_default();
        return Err(SyncError::R2Upload {
            key: file_path.display().to_string(),
            source: format!("HTTP {status}: {body_text}").into(),
        });
    }

    progress_cb(total_bytes, total_bytes);
    info!("Upload complete: {}", file_path.display());
    Ok(())
}

// ── Multipart Upload ─────────────────────────────────────────────────────────

/// Upload a large file (>= 100 MiB) using multipart upload via presigned URLs.
///
/// Parts are uploaded with up to `MAX_CONCURRENT_PARTS` (2) in parallel.
/// The `already_completed` slice allows resuming after a crash — those part
/// indices will be skipped.
///
/// The `progress_cb` is called with `(bytes_uploaded, total_bytes)`.
pub async fn upload_multipart<F>(
    client: &Client,
    urls: &MultipartUrls,
    file_path: &Path,
    already_completed: &[u32],
    progress_cb: F,
) -> Result<UploadProgress, SyncError>
where
    F: Fn(u64, u64) + Send + Sync + 'static,
{
    let metadata = tokio::fs::metadata(file_path).await.map_err(|e| SyncError::FileSystem {
        path: file_path.display().to_string(),
        source: e,
    })?;
    let total_bytes = metadata.len();
    let total_parts = urls.part_urls.len() as u32;

    info!(
        "Multipart upload: {} ({} bytes, {} parts, {} already done)",
        file_path.display(),
        total_bytes,
        total_parts,
        already_completed.len()
    );

    // Track completed parts, seeding with any already-completed ones.
    let completed = Arc::new(tokio::sync::Mutex::new(already_completed.to_vec()));

    // Track total bytes uploaded so far (including already-completed parts).
    let bytes_uploaded = Arc::new(std::sync::atomic::AtomicU64::new(
        already_completed.len() as u64 * PART_SIZE,
    ));

    let progress_cb = Arc::new(progress_cb);

    // Semaphore to limit concurrent part uploads.
    let part_semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_PARTS));

    // Upload each part that hasn't been completed yet.
    let mut handles = Vec::new();

    for part_index in 0..total_parts {
        if already_completed.contains(&part_index) {
            debug!("Skipping already-completed part {part_index}");
            continue;
        }

        let client = client.clone();
        let part_url = urls.part_urls[part_index as usize].clone();
        let file_path = file_path.to_path_buf();
        let sem = Arc::clone(&part_semaphore);
        let completed = Arc::clone(&completed);
        let bytes_uploaded = Arc::clone(&bytes_uploaded);
        let progress_cb = Arc::clone(&progress_cb);

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.map_err(|e| {
                SyncError::Other(format!("Semaphore acquire failed: {e}"))
            })?;

            let part_data = read_part(&file_path, part_index, total_bytes).await?;
            let part_len = part_data.len() as u64;

            let response = client
                .put(&part_url)
                .header("content-length", part_len)
                .body(part_data)
                .send()
                .await?;

            if !response.status().is_success() {
                let status = response.status();
                let body_text = response.text().await.unwrap_or_default();
                return Err(SyncError::R2Upload {
                    key: format!("{}:part-{}", file_path.display(), part_index),
                    source: format!("HTTP {status}: {body_text}").into(),
                });
            }

            // Track progress.
            let uploaded = bytes_uploaded
                .fetch_add(part_len, std::sync::atomic::Ordering::Relaxed)
                + part_len;
            progress_cb(uploaded, total_bytes);

            // Record this part as completed.
            completed.lock().await.push(part_index);

            debug!("Part {part_index} uploaded ({part_len} bytes)");
            Ok::<(), SyncError>(())
        });

        handles.push(handle);
    }

    // Await all part uploads.
    for handle in handles {
        handle
            .await
            .map_err(|e| SyncError::Other(format!("Part upload task panicked: {e}")))??;
    }

    // Complete the multipart upload.
    let response = client.post(&urls.complete_url).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let body_text = response.text().await.unwrap_or_default();
        return Err(SyncError::R2Upload {
            key: file_path.display().to_string(),
            source: format!("Complete multipart HTTP {status}: {body_text}").into(),
        });
    }

    let final_completed = completed.lock().await.clone();
    progress_cb(total_bytes, total_bytes);

    info!("Multipart upload complete: {}", file_path.display());

    Ok(UploadProgress {
        parts_completed: final_completed,
        total_parts,
    })
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Read a single part's worth of data from a file.
async fn read_part(file_path: &Path, part_index: u32, file_size: u64) -> Result<Vec<u8>, SyncError> {
    let offset = part_index as u64 * PART_SIZE;
    let remaining = file_size.saturating_sub(offset);
    let part_len = remaining.min(PART_SIZE) as usize;

    let file = tokio::fs::File::open(file_path).await.map_err(|e| SyncError::FileSystem {
        path: file_path.display().to_string(),
        source: e,
    })?;

    let mut reader = BufReader::new(file);

    // Seek to the correct offset.
    use tokio::io::AsyncSeekExt;
    reader
        .seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| SyncError::FileSystem {
            path: file_path.display().to_string(),
            source: e,
        })?;

    let mut buf = vec![0u8; part_len];
    reader.read_exact(&mut buf).await.map_err(|e| SyncError::FileSystem {
        path: file_path.display().to_string(),
        source: e,
    })?;

    Ok(buf)
}
