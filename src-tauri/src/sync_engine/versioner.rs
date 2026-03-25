//! Version manifest creation and diffing.
//!
//! A **manifest** is a JSON snapshot of every file in the session at a point
//! in time.  It maps relative file paths to their BLAKE3 content hashes and
//! sizes.  Manifests are stored in R2 at:
//!
//!     `_versions/{session_uuid}/v{NNN}/manifest.json`
//!
//! The manifest is the definitive record of what files at what hashes make
//! up a version.  Rollback works by reading a manifest and downloading the
//! referenced content-addressed objects from `_objects/{blake3_hash}`.
//!
//! Manifests are **never overwritten** — each version gets its own manifest.

use std::collections::HashMap;

use chrono::Utc;
use serde_json::json;
use tracing::{debug, info};

use crate::error::{SyncError, SyncResult};
use crate::state::models::{Manifest, ManifestFileEntry, VersionDiff};

use super::checkout::ConvexClient;
use super::hasher;
use super::scanner::FileEntry;

// ── Manifest creation ───────────────────────────────────────────────────

/// Build a `Manifest` from the current set of file entries.
///
/// The manifest captures the exact state of the session at this moment:
/// every file path mapped to its BLAKE3 hash (which is the R2 object key
/// under `_objects/`) and size.
pub fn create_manifest(session_id: &str, version: u64, entries: &[FileEntry]) -> Manifest {
    let mut files = HashMap::with_capacity(entries.len());

    for entry in entries {
        files.insert(
            entry.relative_path.clone(),
            ManifestFileEntry {
                blake3_hash: entry.blake3_hash.clone(),
                size_bytes: entry.size,
            },
        );
    }

    Manifest {
        version,
        session_id: session_id.to_string(),
        created_at: Utc::now().to_rfc3339(),
        files,
    }
}

/// Serialize a manifest to a pretty-printed JSON string.
pub fn serialize_manifest(manifest: &Manifest) -> SyncResult<String> {
    serde_json::to_string_pretty(manifest).map_err(SyncError::Serialization)
}

/// Deserialize a manifest from a JSON string.
pub fn deserialize_manifest(json_str: &str) -> SyncResult<Manifest> {
    serde_json::from_str(json_str).map_err(SyncError::Serialization)
}

// ── Manifest diffing ────────────────────────────────────────────────────

/// Compute the diff between two manifests (typically the previous version
/// and the current version).
///
/// Returns a `VersionDiff` listing added, modified, and deleted file paths.
pub fn diff_manifests(old: &Manifest, new: &Manifest) -> VersionDiff {
    let mut diff = VersionDiff::default();

    // Files in new but not in old -> added.
    // Files in both but with different hashes -> modified.
    for (path, new_entry) in &new.files {
        match old.files.get(path) {
            None => {
                diff.added.push(path.clone());
            }
            Some(old_entry) => {
                if old_entry.blake3_hash != new_entry.blake3_hash {
                    diff.modified.push(path.clone());
                }
            }
        }
    }

    // Files in old but not in new -> deleted.
    for path in old.files.keys() {
        if !new.files.contains_key(path) {
            diff.deleted.push(path.clone());
        }
    }

    // Sort for deterministic output (helps with testing and logging).
    diff.added.sort();
    diff.modified.sort();
    diff.deleted.sort();

    debug!(
        added = diff.added.len(),
        modified = diff.modified.len(),
        deleted = diff.deleted.len(),
        "Manifest diff computed"
    );

    diff
}

// ── Auto-summary generation ─────────────────────────────────────────────

/// Generate a human-readable summary of a version diff.
///
/// Examples:
/// - "3 new audio files, .ptx modified"
/// - "session.ptx modified, 1 audio file deleted"
/// - "12 new files, 2 modified, 1 deleted"
/// - "No changes"
pub fn generate_auto_summary(diff: &VersionDiff) -> String {
    if diff.added.is_empty() && diff.modified.is_empty() && diff.deleted.is_empty() {
        return "No changes".to_string();
    }

    let mut parts = Vec::new();

    // ── Added files ─────────────────────────────────────────────────
    if !diff.added.is_empty() {
        let audio_count = count_by_type(&diff.added, is_audio_file);
        let ptx_count = count_by_type(&diff.added, is_ptx_file);
        let other_count = diff.added.len() - audio_count - ptx_count;

        if audio_count > 0 {
            parts.push(format!(
                "{} new audio {}",
                audio_count,
                pluralize("file", audio_count)
            ));
        }
        if ptx_count > 0 {
            parts.push(".ptx added".to_string());
        }
        if other_count > 0 {
            parts.push(format!(
                "{} new {}",
                other_count,
                pluralize("file", other_count)
            ));
        }
    }

    // ── Modified files ──────────────────────────────────────────────
    if !diff.modified.is_empty() {
        let ptx_modified = diff.modified.iter().any(|p| is_ptx_file(p));
        let audio_modified = count_by_type(&diff.modified, is_audio_file);
        let other_modified = diff.modified.len()
            - audio_modified
            - if ptx_modified { 1 } else { 0 };

        if ptx_modified {
            parts.push(".ptx modified".to_string());
        }
        if audio_modified > 0 {
            parts.push(format!(
                "{} audio {} modified",
                audio_modified,
                pluralize("file", audio_modified)
            ));
        }
        if other_modified > 0 {
            parts.push(format!(
                "{} {} modified",
                other_modified,
                pluralize("file", other_modified)
            ));
        }
    }

    // ── Deleted files ───────────────────────────────────────────────
    if !diff.deleted.is_empty() {
        let audio_deleted = count_by_type(&diff.deleted, is_audio_file);
        let other_deleted = diff.deleted.len() - audio_deleted;

        if audio_deleted > 0 {
            parts.push(format!(
                "{} audio {} deleted",
                audio_deleted,
                pluralize("file", audio_deleted)
            ));
        }
        if other_deleted > 0 {
            parts.push(format!(
                "{} {} deleted",
                other_deleted,
                pluralize("file", other_deleted)
            ));
        }
    }

    parts.join(", ")
}

// ── R2 manifest upload ──────────────────────────────────────────────────

/// Upload a version manifest to R2 via a presigned URL obtained from Convex.
///
/// The manifest is stored at `_versions/{session_uuid}/v{NNN}/manifest.json`.
/// After uploading the manifest, this function calls the Convex `createVersion`
/// mutation to record the version in the control plane.
pub async fn upload_manifest(
    manifest: &Manifest,
    session_uuid: &str,
    machine_id: &str,
    auto_summary: &str,
    release_note: Option<&str>,
    is_release: bool,
    files_changed: u64,
    bytes_changed: i64,
    convex: &ConvexClient,
) -> SyncResult<()> {
    let manifest_json = serialize_manifest(manifest)?;
    let manifest_bytes = manifest_json.as_bytes();
    let manifest_hash = hasher::hash_bytes(manifest_bytes);
    let version = manifest.version;

    let r2_key = format!(
        "_versions/{}/v{:03}/manifest.json",
        session_uuid, version
    );

    info!(
        session_uuid = session_uuid,
        version = version,
        r2_key = %r2_key,
        manifest_size = manifest_bytes.len(),
        "Uploading version manifest"
    );

    // Step 1: Request a presigned upload URL from Convex.
    let presigned_response = convex
        .action(
            "presignedUrls:requestUploadUrl",
            json!({
                "sessionId": session_uuid,
                "objectKey": r2_key,
                "contentType": "application/json",
                "machineId": machine_id,
            }),
        )
        .await?;

    let upload_url = presigned_response
        .get("value")
        .and_then(|v| v.get("url"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyncError::ConvexApi {
            function: "presignedUrls:requestUploadUrl".to_string(),
            message: "Missing upload URL in response".to_string(),
        })?;

    // Step 2: Upload the manifest JSON to R2 via the presigned URL.
    let http_client = reqwest::Client::new();
    let response = http_client
        .put(upload_url)
        .header("Content-Type", "application/json")
        .body(manifest_json.clone())
        .send()
        .await
        .map_err(SyncError::Http)?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(SyncError::ConvexApi {
            function: "R2 manifest upload".to_string(),
            message: format!("Upload failed with status {}: {}", status, body),
        });
    }

    info!(
        r2_key = %r2_key,
        "Manifest uploaded to R2 successfully"
    );

    // Step 3: Create the version record in Convex.
    convex
        .mutation(
            "versions:createVersion",
            json!({
                "sessionId": session_uuid,
                "versionNumber": version,
                "pushedBy": machine_id,
                "autoSummary": auto_summary,
                "releaseNote": release_note,
                "isRelease": is_release,
                "filesChanged": files_changed,
                "bytesChanged": bytes_changed,
                "r2ManifestKey": r2_key,
            }),
        )
        .await?;

    info!(
        session_uuid = session_uuid,
        version = version,
        is_release = is_release,
        "Version record created in Convex"
    );

    Ok(())
}

/// Download and parse a manifest from R2 for a specific version.
pub async fn download_manifest(
    session_uuid: &str,
    version: u64,
    machine_id: &str,
    convex: &ConvexClient,
) -> SyncResult<Manifest> {
    let r2_key = format!(
        "_versions/{}/v{:03}/manifest.json",
        session_uuid, version
    );

    info!(
        r2_key = %r2_key,
        "Downloading version manifest"
    );

    // Request presigned download URL.
    let presigned_response = convex
        .action(
            "presignedUrls:requestDownloadUrl",
            json!({
                "sessionId": session_uuid,
                "objectKey": r2_key,
                "machineId": machine_id,
            }),
        )
        .await?;

    let download_url = presigned_response
        .get("value")
        .and_then(|v| v.get("url"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyncError::ConvexApi {
            function: "presignedUrls:requestDownloadUrl".to_string(),
            message: "Missing download URL in response".to_string(),
        })?;

    // Download the manifest.
    let http_client = reqwest::Client::new();
    let response = http_client
        .get(download_url)
        .send()
        .await
        .map_err(SyncError::Http)?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(SyncError::ConvexApi {
            function: "R2 manifest download".to_string(),
            message: format!("Download failed with status {}: {}", status, body),
        });
    }

    let json_str = response.text().await.map_err(SyncError::Http)?;
    let manifest = deserialize_manifest(&json_str)?;

    info!(
        version = manifest.version,
        files = manifest.files.len(),
        "Manifest downloaded and parsed"
    );

    Ok(manifest)
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn is_audio_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".wav")
        || lower.ends_with(".aiff")
        || lower.ends_with(".aif")
        || lower.ends_with(".mp3")
        || lower.ends_with(".flac")
        || lower.ends_with(".ogg")
}

fn is_ptx_file(path: &str) -> bool {
    path.to_lowercase().ends_with(".ptx")
}

fn count_by_type(paths: &[String], predicate: fn(&str) -> bool) -> usize {
    paths.iter().filter(|p| predicate(p)).count()
}

fn pluralize(word: &str, count: usize) -> &str {
    if count == 1 {
        word
    } else {
        // Simple English pluralization — only "file" -> "files" is needed here.
        // Return a &str by matching known words.
        match word {
            "file" => "files",
            _ => word,
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_entries() -> Vec<FileEntry> {
        vec![
            FileEntry {
                relative_path: "Audio Files/kick.wav".to_string(),
                absolute_path: PathBuf::from("/session/Audio Files/kick.wav"),
                size: 1_000_000,
                mtime: 1000,
                blake3_hash: "aaaa".to_string(),
            },
            FileEntry {
                relative_path: "Audio Files/snare.wav".to_string(),
                absolute_path: PathBuf::from("/session/Audio Files/snare.wav"),
                size: 2_000_000,
                mtime: 2000,
                blake3_hash: "bbbb".to_string(),
            },
            FileEntry {
                relative_path: "session.ptx".to_string(),
                absolute_path: PathBuf::from("/session/session.ptx"),
                size: 50_000_000,
                mtime: 3000,
                blake3_hash: "cccc".to_string(),
            },
        ]
    }

    #[test]
    fn test_create_manifest() {
        let entries = make_entries();
        let manifest = create_manifest("sess1", 1, &entries);

        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.session_id, "sess1");
        assert_eq!(manifest.files.len(), 3);
        assert_eq!(
            manifest.files["Audio Files/kick.wav"].blake3_hash,
            "aaaa"
        );
        assert_eq!(
            manifest.files["Audio Files/kick.wav"].size_bytes,
            1_000_000
        );
    }

    #[test]
    fn test_serialize_deserialize_manifest() {
        let entries = make_entries();
        let manifest = create_manifest("sess1", 42, &entries);

        let json = serialize_manifest(&manifest).unwrap();
        assert!(json.contains("\"version\": 42"));
        assert!(json.contains("kick.wav"));

        let deserialized = deserialize_manifest(&json).unwrap();
        assert_eq!(deserialized.version, 42);
        assert_eq!(deserialized.files.len(), 3);
        assert_eq!(
            deserialized.files["session.ptx"].blake3_hash,
            "cccc"
        );
    }

    #[test]
    fn test_diff_manifests_all_new() {
        let empty = Manifest {
            version: 0,
            session_id: "sess1".to_string(),
            created_at: Utc::now().to_rfc3339(),
            files: HashMap::new(),
        };
        let entries = make_entries();
        let current = create_manifest("sess1", 1, &entries);

        let diff = diff_manifests(&empty, &current);
        assert_eq!(diff.added.len(), 3);
        assert!(diff.modified.is_empty());
        assert!(diff.deleted.is_empty());
    }

    #[test]
    fn test_diff_manifests_modification() {
        let entries = make_entries();
        let old = create_manifest("sess1", 1, &entries);

        // Modify the ptx hash.
        let mut new_entries = make_entries();
        new_entries[2].blake3_hash = "dddd".to_string();
        let new = create_manifest("sess1", 2, &new_entries);

        let diff = diff_manifests(&old, &new);
        assert!(diff.added.is_empty());
        assert_eq!(diff.modified, vec!["session.ptx".to_string()]);
        assert!(diff.deleted.is_empty());
    }

    #[test]
    fn test_diff_manifests_deletion() {
        let entries = make_entries();
        let old = create_manifest("sess1", 1, &entries);

        // Remove one audio file.
        let new_entries = vec![entries[0].clone(), entries[2].clone()];
        let new = create_manifest("sess1", 2, &new_entries);

        let diff = diff_manifests(&old, &new);
        assert!(diff.added.is_empty());
        assert!(diff.modified.is_empty());
        assert_eq!(diff.deleted, vec!["Audio Files/snare.wav".to_string()]);
    }

    #[test]
    fn test_diff_manifests_mixed() {
        let entries = make_entries();
        let old = create_manifest("sess1", 1, &entries);

        // Add a new file, modify ptx, delete snare.
        let mut new_entries = vec![
            entries[0].clone(),
            FileEntry {
                relative_path: "Audio Files/bass.wav".to_string(),
                absolute_path: PathBuf::from("/session/Audio Files/bass.wav"),
                size: 3_000_000,
                mtime: 4000,
                blake3_hash: "eeee".to_string(),
            },
            FileEntry {
                relative_path: "session.ptx".to_string(),
                absolute_path: PathBuf::from("/session/session.ptx"),
                size: 50_000_000,
                mtime: 5000,
                blake3_hash: "ffff".to_string(), // changed hash
            },
        ];
        let new = create_manifest("sess1", 2, &new_entries);

        let diff = diff_manifests(&old, &new);
        assert_eq!(diff.added, vec!["Audio Files/bass.wav".to_string()]);
        assert_eq!(diff.modified, vec!["session.ptx".to_string()]);
        assert_eq!(diff.deleted, vec!["Audio Files/snare.wav".to_string()]);
    }

    #[test]
    fn test_generate_auto_summary_no_changes() {
        let diff = VersionDiff::default();
        assert_eq!(generate_auto_summary(&diff), "No changes");
    }

    #[test]
    fn test_generate_auto_summary_new_audio() {
        let diff = VersionDiff {
            added: vec![
                "Audio Files/kick.wav".to_string(),
                "Audio Files/snare.wav".to_string(),
                "Audio Files/hat.wav".to_string(),
            ],
            modified: vec!["session.ptx".to_string()],
            deleted: vec![],
        };

        let summary = generate_auto_summary(&diff);
        assert!(summary.contains("3 new audio files"));
        assert!(summary.contains(".ptx modified"));
    }

    #[test]
    fn test_generate_auto_summary_deleted() {
        let diff = VersionDiff {
            added: vec![],
            modified: vec![],
            deleted: vec!["Audio Files/old.wav".to_string()],
        };

        let summary = generate_auto_summary(&diff);
        assert!(summary.contains("1 audio file deleted"));
    }

    #[test]
    fn test_generate_auto_summary_single_audio() {
        let diff = VersionDiff {
            added: vec!["Audio Files/vocal.wav".to_string()],
            modified: vec![],
            deleted: vec![],
        };

        let summary = generate_auto_summary(&diff);
        assert_eq!(summary, "1 new audio file");
    }
}
