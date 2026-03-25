pub mod download;
pub mod trash;
pub mod upload;

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use reqwest::Client;
use tokio::sync::{Mutex, Semaphore};
use tracing::{debug, info};

use crate::error::SyncError;

pub use download::{download_file, DownloadProgress};
pub use trash::{trash_file, TrashEntry, TrashOperation};
pub use upload::{upload_file, upload_multipart, MultipartUrls, UploadProgress, MULTIPART_THRESHOLD};

// ── Upload Priority ──────────────────────────────────────────────────────────

/// Priority levels for the upload queue. Lower numeric value = higher priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadPriority {
    /// Currently checked-out session — highest priority.
    CheckedOut = 0,
    /// Background sync for non-checked-out sessions.
    Background = 1,
}

/// An item in the priority upload queue.
#[derive(Debug, Clone)]
pub struct UploadJob {
    pub priority: UploadPriority,
    pub session_id: String,
    pub file_path: PathBuf,
    pub presigned_url: String,
    /// For multipart uploads; None means single-part.
    pub multipart_urls: Option<MultipartUrls>,
    /// File size in bytes (used to decide single vs multipart).
    pub file_size: u64,
    /// Sequence number for FIFO ordering within the same priority.
    pub sequence: u64,
}

// Implement Ord so BinaryHeap gives us highest-priority (lowest enum value)
// first, with FIFO tiebreaking via sequence number.
impl PartialOrd for UploadJob {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for UploadJob {
    fn cmp(&self, other: &Self) -> Ordering {
        // Lower priority value = higher priority in the heap.
        let pri = (self.priority as u8).cmp(&(other.priority as u8)).reverse();
        if pri != Ordering::Equal {
            return pri;
        }
        // Within the same priority, earlier sequence = higher priority.
        self.sequence.cmp(&other.sequence).reverse()
    }
}

impl Eq for UploadJob {}

impl PartialEq for UploadJob {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.sequence == other.sequence
    }
}

// ── R2 Client ────────────────────────────────────────────────────────────────

/// Central R2 client that orchestrates uploads and downloads via presigned URLs.
///
/// All R2 operations go through presigned URLs issued by the Convex backend.
/// No raw R2 credentials are stored or used on the client.
pub struct R2Client {
    /// HTTP client for all R2 requests.
    client: Client,
    /// Bucket name (used for logging and key construction, not for direct access).
    bucket_name: String,
    /// Global semaphore limiting concurrent uploads to 2.
    upload_semaphore: Arc<Semaphore>,
    /// Priority queue of pending upload jobs.
    upload_queue: Arc<Mutex<BinaryHeap<UploadJob>>>,
    /// Monotonically increasing sequence counter for FIFO ordering.
    sequence_counter: Arc<std::sync::atomic::AtomicU64>,
}

impl R2Client {
    /// Create a new R2Client for the given bucket.
    pub fn new(bucket_name: String) -> Self {
        let client = Client::builder()
            .pool_max_idle_per_host(4)
            .build()
            .expect("Failed to build reqwest client");

        Self {
            client,
            bucket_name,
            upload_semaphore: Arc::new(Semaphore::new(2)),
            upload_queue: Arc::new(Mutex::new(BinaryHeap::new())),
            sequence_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Returns a reference to the bucket name.
    pub fn bucket_name(&self) -> &str {
        &self.bucket_name
    }

    /// Returns a reference to the inner reqwest Client (useful for direct
    /// calls to trash or download that don't go through the queue).
    pub fn http_client(&self) -> &Client {
        &self.client
    }

    // ── Upload Operations ────────────────────────────────────────────────

    /// Enqueue an upload job. The job will be processed respecting priority
    /// ordering and the global concurrency limit (max 2 concurrent uploads).
    pub async fn enqueue_upload(&self, job: UploadJob) {
        let mut queue = self.upload_queue.lock().await;
        debug!(
            "Enqueuing upload: session={}, file={}, priority={:?}",
            job.session_id,
            job.file_path.display(),
            job.priority
        );
        queue.push(job);
    }

    /// Create an upload job with the next sequence number.
    pub fn create_upload_job(
        &self,
        priority: UploadPriority,
        session_id: String,
        file_path: PathBuf,
        presigned_url: String,
        multipart_urls: Option<MultipartUrls>,
        file_size: u64,
    ) -> UploadJob {
        let sequence = self
            .sequence_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        UploadJob {
            priority,
            session_id,
            file_path,
            presigned_url,
            multipart_urls,
            file_size,
            sequence,
        }
    }

    /// Process the next upload job from the priority queue. Returns `None` if
    /// the queue is empty. Acquires a permit from the global upload semaphore
    /// before starting the upload.
    ///
    /// The `progress_cb` receives `(bytes_uploaded, total_bytes)`.
    pub async fn process_next_upload<F>(
        &self,
        progress_cb: F,
    ) -> Option<Result<(), SyncError>>
    where
        F: Fn(u64, u64) + Send + Sync + 'static,
    {
        // Pop the highest-priority job.
        let job = {
            let mut queue = self.upload_queue.lock().await;
            queue.pop()
        }?;

        info!(
            "Processing upload: session={}, file={}, priority={:?}, size={}",
            job.session_id,
            job.file_path.display(),
            job.priority,
            job.file_size
        );

        // Acquire global upload permit (blocks until a slot is free).
        let _permit = self
            .upload_semaphore
            .acquire()
            .await
            .map_err(|e| SyncError::Other(format!("Upload semaphore closed: {e}")));

        let _permit = match _permit {
            Ok(p) => p,
            Err(e) => return Some(Err(e)),
        };

        // Choose single or multipart based on file size and available URLs.
        let result = if job.file_size >= MULTIPART_THRESHOLD {
            if let Some(ref urls) = job.multipart_urls {
                upload_multipart(&self.client, urls, &job.file_path, &[], progress_cb)
                    .await
                    .map(|_| ())
            } else {
                // Fallback to single upload if no multipart URLs were provided.
                upload_file(&self.client, &job.presigned_url, &job.file_path, progress_cb).await
            }
        } else {
            upload_file(&self.client, &job.presigned_url, &job.file_path, progress_cb).await
        };

        Some(result)
    }

    /// Drain and process all queued uploads, respecting priority and the global
    /// concurrency limit. Returns the number of successful uploads and a list
    /// of any errors encountered.
    pub async fn process_all_uploads<F>(
        &self,
        progress_cb_factory: F,
    ) -> (usize, Vec<SyncError>)
    where
        F: Fn() -> Box<dyn Fn(u64, u64) + Send + Sync + 'static>,
    {
        let mut successes = 0usize;
        let mut errors = Vec::new();

        loop {
            let cb = progress_cb_factory();
            match self.process_next_upload(cb).await {
                Some(Ok(())) => successes += 1,
                Some(Err(e)) => errors.push(e),
                None => break, // queue empty
            }
        }

        info!(
            "Upload queue drained: {} succeeded, {} failed",
            successes,
            errors.len()
        );
        (successes, errors)
    }

    /// Return the number of jobs currently in the upload queue.
    pub async fn upload_queue_len(&self) -> usize {
        self.upload_queue.lock().await.len()
    }

    // ── Download Operations ──────────────────────────────────────────────

    /// Download a file from a presigned URL, verify its BLAKE3 hash, and
    /// atomically place it at `dest_path`.
    pub async fn download<F>(
        &self,
        presigned_url: &str,
        dest_path: &Path,
        expected_hash: &str,
        progress_cb: F,
    ) -> Result<(), SyncError>
    where
        F: Fn(u64, u64) + Send + Sync + 'static,
    {
        download_file(&self.client, presigned_url, dest_path, expected_hash, progress_cb).await
    }

    // ── Trash Operations ─────────────────────────────────────────────────

    /// Soft-delete a single file by copying it to the `_trash/` prefix and
    /// deleting the original, both via presigned URLs.
    pub async fn trash(
        &self,
        presigned_copy_url: &str,
        presigned_delete_url: &str,
    ) -> Result<(), SyncError> {
        trash_file(&self.client, presigned_copy_url, presigned_delete_url).await
    }

    /// Batch soft-delete. Best-effort — individual failures do not stop the
    /// batch.
    pub async fn trash_batch(
        &self,
        operations: Vec<TrashOperation>,
    ) -> (Vec<TrashEntry>, Vec<SyncError>) {
        trash::trash_files(&self.client, operations).await
    }
}
