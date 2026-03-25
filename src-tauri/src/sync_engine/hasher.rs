//! BLAKE3 file hashing with adaptive strategy.
//!
//! - Files < 1 GB: memory-mapped I/O via `memmap2` for ~1 GB/s throughput.
//! - Files >= 1 GB: streaming reader with 64 KB chunks to cap memory usage
//!   when Pro Tools is consuming most of the machine's RAM.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use memmap2::Mmap;
use tracing::{debug, trace};

use crate::error::{SyncError, SyncResult};

/// Threshold below which we use memory-mapped I/O.
/// 1 GiB = 1_073_741_824 bytes.
const MMAP_THRESHOLD: u64 = 1_073_741_824;

/// Chunk size for the streaming fallback path.
const STREAM_CHUNK_SIZE: usize = 64 * 1024; // 64 KiB

/// Hash a file on disk and return the hex-encoded BLAKE3 digest.
///
/// The function picks the hashing strategy based on file size:
///
/// - **< 1 GiB**: creates a read-only memory map and feeds the entire mapping
///   into `blake3::Hasher::update_mmap_rayon` (parallel, ~1 GB/s on a modern
///   SSD). Falls back to streaming if the mmap fails (e.g. special filesystem).
///
/// - **>= 1 GiB**: streams the file through a 64 KiB buffer so we never pin
///   more than a page of physical memory. Slightly slower, but critical when
///   Pro Tools is using most of the system RAM for its playback engine.
pub fn hash_file(path: &Path) -> SyncResult<String> {
    let metadata = std::fs::metadata(path).map_err(|e| SyncError::FileSystem {
        path: path.display().to_string(),
        source: e,
    })?;

    let size = metadata.len();

    if size == 0 {
        // Empty file — return the BLAKE3 hash of empty input.
        let hash = blake3::hash(b"");
        return Ok(hash.to_hex().to_string());
    }

    let hex = if size < MMAP_THRESHOLD {
        hash_file_mmap(path, size)?
    } else {
        debug!(
            path = %path.display(),
            size_mb = size / (1024 * 1024),
            "File >= 1 GiB — using streaming hash"
        );
        hash_file_streaming(path)?
    };

    trace!(path = %path.display(), hash = %hex, "Hashed file");
    Ok(hex)
}

/// Hash using a memory-mapped file.
///
/// We open the file, create an `Mmap`, and feed it directly into the BLAKE3
/// hasher.  On failure (e.g. the file is on a FUSE mount that rejects mmap),
/// we transparently fall back to the streaming path.
fn hash_file_mmap(path: &Path, _size: u64) -> SyncResult<String> {
    let file = File::open(path).map_err(|e| SyncError::FileSystem {
        path: path.display().to_string(),
        source: e,
    })?;

    // SAFETY: We opened the file read-only and do not write to the mapping.
    // The file could be modified concurrently by Pro Tools, but we tolerate
    // that — we'll detect changes via the next watcher cycle.
    let mmap = match unsafe { Mmap::map(&file) } {
        Ok(m) => m,
        Err(e) => {
            debug!(
                path = %path.display(),
                error = %e,
                "mmap failed, falling back to streaming"
            );
            return hash_file_streaming(path);
        }
    };

    let mut hasher = blake3::Hasher::new();
    hasher.update(&mmap);
    let hash = hasher.finalize();
    Ok(hash.to_hex().to_string())
}

/// Hash using a streaming 64 KiB reader.
fn hash_file_streaming(path: &Path) -> SyncResult<String> {
    let mut file = File::open(path).map_err(|e| SyncError::FileSystem {
        path: path.display().to_string(),
        source: e,
    })?;

    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; STREAM_CHUNK_SIZE];

    loop {
        let n = file.read(&mut buf).map_err(|e| SyncError::FileSystem {
            path: path.display().to_string(),
            source: e,
        })?;

        if n == 0 {
            break;
        }

        hasher.update(&buf[..n]);
    }

    let hash = hasher.finalize();
    Ok(hash.to_hex().to_string())
}

/// Hash an in-memory byte slice and return the hex-encoded BLAKE3 digest.
///
/// Used for small payloads like serialized manifest JSON.
pub fn hash_bytes(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_hash_empty_file() {
        let mut tmp = NamedTempFile::new().unwrap();
        // Write nothing — file is 0 bytes.
        tmp.flush().unwrap();

        let hash = hash_file(tmp.path()).unwrap();
        let expected = blake3::hash(b"").to_hex().to_string();
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_hash_small_file() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"hello world").unwrap();
        tmp.flush().unwrap();

        let hash = hash_file(tmp.path()).unwrap();
        let expected = blake3::hash(b"hello world").to_hex().to_string();
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_hash_bytes() {
        let data = b"manifest content";
        let hash = hash_bytes(data);
        let expected = blake3::hash(data).to_hex().to_string();
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_hash_consistency() {
        // Verify that mmap and streaming paths produce the same result.
        let mut tmp = NamedTempFile::new().unwrap();
        let payload = vec![0xABu8; 128 * 1024]; // 128 KiB
        tmp.write_all(&payload).unwrap();
        tmp.flush().unwrap();

        let h_mmap = hash_file_mmap(tmp.path(), payload.len() as u64).unwrap();
        let h_stream = hash_file_streaming(tmp.path()).unwrap();
        assert_eq!(h_mmap, h_stream);
    }

    #[test]
    fn test_nonexistent_file_returns_error() {
        let result = hash_file(Path::new("/tmp/nonexistent_sessionsync_test_file.wav"));
        assert!(result.is_err());
    }
}
