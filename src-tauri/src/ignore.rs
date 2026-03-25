//! Pro Tools ignore-pattern filter.
//!
//! Determines which files and directories inside a session folder should be
//! excluded from syncing. Ships with sensible defaults for Pro Tools and
//! supports user-defined patterns using gitignore-style glob syntax.

use std::path::Path;

use glob::Pattern;
use tracing::warn;

/// A compiled set of ignore patterns.
///
/// Patterns are matched against the **relative** path within the session
/// folder. Both file names and directory components are tested — a pattern
/// ending with `/` matches only directories.
#[derive(Debug, Clone)]
pub struct IgnoreFilter {
    /// Compiled glob patterns.
    patterns: Vec<CompiledPattern>,
}

#[derive(Debug, Clone)]
struct CompiledPattern {
    /// The original user-facing pattern string (for diagnostics).
    raw: String,
    /// Compiled glob.
    glob: Pattern,
    /// If `true`, the pattern should only match directories (not leaf files).
    dir_only: bool,
}

/// Default ignore patterns for Pro Tools session folders.
///
/// These match the table in the PRD (`06-pro-tools.md`):
///
/// | Pattern                      | Reason                               |
/// |------------------------------|--------------------------------------|
/// | `Session File Backups/`      | PT auto-backups, regenerable         |
/// | `WaveCache.wfm`              | Peak cache, regenerated on open      |
/// | `*.pkf`                      | Peak cache files, regenerated        |
/// | `.DS_Store`                  | macOS metadata                       |
/// | `*.sessionsync-tmp`          | Temp files during downloads          |
/// | `.Spotlight-V100/`           | macOS Spotlight index                |
/// | `.Trashes/`                  | macOS Trash folder                   |
const DEFAULT_PATTERNS: &[&str] = &[
    "Session File Backups/",
    "WaveCache.wfm",
    "*.pkf",
    ".DS_Store",
    "*.sessionsync-tmp",
    ".Spotlight-V100/",
    ".Trashes/",
    "Thumbs.db",
];

impl IgnoreFilter {
    /// Build a new filter with only the built-in defaults (no custom patterns).
    pub fn new() -> Self {
        let mut filter = IgnoreFilter {
            patterns: Vec::with_capacity(DEFAULT_PATTERNS.len()),
        };
        for pat in DEFAULT_PATTERNS {
            filter.add_pattern(pat);
        }
        filter
    }

    /// Build a filter from defaults plus a set of user-defined custom patterns.
    pub fn with_custom_patterns(custom: &[String]) -> Self {
        let mut filter = Self::new();
        for pat in custom {
            filter.add_pattern(pat);
        }
        filter
    }

    /// Add a single gitignore-style pattern.
    ///
    /// - Patterns ending with `/` match directories only.
    /// - Leading `/` is stripped (all patterns are relative to session root).
    /// - Standard glob wildcards (`*`, `?`, `[...]`) are supported.
    /// - Lines starting with `#` and blank lines are silently ignored.
    pub fn add_pattern(&mut self, raw: &str) {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            // Empty lines and comments are ignored (gitignore convention).
            return;
        }

        let dir_only = trimmed.ends_with('/');
        // Strip leading/trailing slashes for glob compilation.
        let glob_str = trimmed.trim_start_matches('/').trim_end_matches('/');

        match Pattern::new(glob_str) {
            Ok(glob) => {
                self.patterns.push(CompiledPattern {
                    raw: trimmed.to_string(),
                    glob,
                    dir_only,
                });
            }
            Err(e) => {
                warn!(pattern = trimmed, error = %e, "invalid ignore pattern — skipping");
            }
        }
    }

    /// Returns `true` if the given path (relative to the session root)
    /// should be ignored (i.e. excluded from syncing).
    ///
    /// The caller must pass a **relative** path. Each component of the path
    /// is tested individually against the patterns, so a directory pattern
    /// like `Session File Backups/` will match any file nested within that
    /// directory.
    pub fn should_ignore(&self, path: &Path) -> bool {
        let components: Vec<String> = path
            .components()
            .filter_map(|c| {
                if let std::path::Component::Normal(os) = c {
                    Some(os.to_string_lossy().to_string())
                } else {
                    None
                }
            })
            .collect();

        let num_components = components.len();

        for (idx, comp) in components.iter().enumerate() {
            let is_last = idx == num_components - 1;
            for pat in &self.patterns {
                if pat.glob.matches(comp) {
                    if pat.dir_only {
                        // Directory-only patterns match when the component is
                        // a parent directory (not the final filename), or when
                        // the path itself IS the directory name.
                        if !is_last || num_components == 1 {
                            return true;
                        }
                    } else {
                        return true;
                    }
                }
            }
        }

        // Also test the full relative path as a string. This allows patterns
        // that include path separators (e.g. "some/nested/*.log").
        let path_str = path.to_string_lossy();
        for pat in &self.patterns {
            if !pat.dir_only && pat.glob.matches(&path_str) {
                return true;
            }
        }

        false
    }

    /// Returns `true` if the path should be ignored.
    ///
    /// Alias for `should_ignore` for backward compatibility.
    pub fn is_ignored(&self, path: &Path) -> bool {
        self.should_ignore(path)
    }

    /// Return the number of active patterns (for diagnostics).
    pub fn pattern_count(&self) -> usize {
        self.patterns.len()
    }
}

impl Default for IgnoreFilter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_ignore_session_backups_dir() {
        let f = IgnoreFilter::new();
        // The directory itself.
        assert!(f.should_ignore(Path::new("Session File Backups")));
        // A file inside the directory.
        assert!(f.should_ignore(Path::new("Session File Backups/backup.ptx")));
    }

    #[test]
    fn test_ignore_wavecache() {
        let f = IgnoreFilter::new();
        assert!(f.should_ignore(Path::new("WaveCache.wfm")));
    }

    #[test]
    fn test_ignore_peak_files() {
        let f = IgnoreFilter::new();
        assert!(f.should_ignore(Path::new("Audio Files/track_01.pkf")));
        assert!(f.should_ignore(Path::new("something.pkf")));
    }

    #[test]
    fn test_ignore_ds_store() {
        let f = IgnoreFilter::new();
        assert!(f.should_ignore(Path::new(".DS_Store")));
        assert!(f.should_ignore(Path::new("Audio Files/.DS_Store")));
    }

    #[test]
    fn test_ignore_tmp_files() {
        let f = IgnoreFilter::new();
        assert!(f.should_ignore(Path::new("track_01.wav.sessionsync-tmp")));
        assert!(f.should_ignore(Path::new(
            "Audio Files/track_01.wav.sessionsync-tmp"
        )));
    }

    #[test]
    fn test_audio_files_are_not_ignored() {
        let f = IgnoreFilter::new();
        assert!(!f.should_ignore(Path::new("Audio Files/track_01.wav")));
        assert!(!f.should_ignore(Path::new("session.ptx")));
        assert!(!f.should_ignore(Path::new("Bounced Files/mix_v3.wav")));
        assert!(!f.should_ignore(Path::new("Video Files/reference.mov")));
    }

    #[test]
    fn test_custom_patterns() {
        let custom = vec!["*.bak".to_string(), "temp/".to_string()];
        let f = IgnoreFilter::with_custom_patterns(&custom);
        assert!(f.should_ignore(Path::new("something.bak")));
        assert!(f.should_ignore(Path::new("temp/file.wav")));
        assert!(!f.should_ignore(Path::new("Audio Files/good.wav")));
    }

    #[test]
    fn test_comments_and_blanks_skipped() {
        let custom = vec![
            "# this is a comment".to_string(),
            "".to_string(),
            "  ".to_string(),
            "*.log".to_string(),
        ];
        let f = IgnoreFilter::with_custom_patterns(&custom);
        // Only *.log plus defaults should be compiled.
        assert_eq!(f.pattern_count(), DEFAULT_PATTERNS.len() + 1);
        assert!(f.should_ignore(Path::new("debug.log")));
    }
}
