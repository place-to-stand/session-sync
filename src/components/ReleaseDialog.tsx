import { useState, useEffect } from "react";
import { formatBytes } from "../lib/convex";
import {
  releaseSession,
  getReleaseSummary,
  type SessionSummary,
} from "../lib/commands";

interface ReleaseDialogProps {
  sessionId: string;
  sessionName: string;
  onRelease: () => void;
  onCancel: () => void;
}

/**
 * Modal dialog shown when releasing a session.
 * Displays an auto-generated summary of changes and allows an optional release note.
 */
export default function ReleaseDialog({
  sessionId,
  sessionName,
  onRelease,
  onCancel,
}: ReleaseDialogProps) {
  const [note, setNote] = useState("");
  const [releasing, setReleasing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [summary, setSummary] = useState<SessionSummary | null>(null);
  const [loadingSummary, setLoadingSummary] = useState(true);

  useEffect(() => {
    let cancelled = false;

    async function loadSummary() {
      try {
        const result = await getReleaseSummary(sessionId);
        if (!cancelled) {
          setSummary(result);
          setLoadingSummary(false);
        }
      } catch {
        if (!cancelled) {
          setSummary(null);
          setLoadingSummary(false);
        }
      }
    }

    loadSummary();
    return () => {
      cancelled = true;
    };
  }, [sessionId]);

  async function handleRelease() {
    setReleasing(true);
    setError(null);
    try {
      await releaseSession(sessionId, note.trim() || undefined);
      onRelease();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to release session");
      setReleasing(false);
    }
  }

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Escape") {
      onCancel();
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60"
      onKeyDown={handleKeyDown}
      onClick={(e) => {
        if (e.target === e.currentTarget) onCancel();
      }}
    >
      <div className="w-[360px] bg-[#2c2c2e] rounded-lg border border-[#3a3a3c] shadow-2xl overflow-hidden">
        {/* Header */}
        <div className="px-4 py-3 border-b border-[#3a3a3c]">
          <h2 className="text-sm font-semibold text-[#f5f5f7]">
            Release {sessionName}
          </h2>
        </div>

        {/* Body */}
        <div className="px-4 py-3 space-y-3">
          {/* Change summary */}
          <div>
            <div className="text-xs text-[#98989d] mb-2">
              Changes since checkout:
            </div>
            {loadingSummary ? (
              <div className="text-xs text-[#636366]">Loading summary...</div>
            ) : summary ? (
              <ul className="space-y-1 text-xs text-[#f5f5f7]">
                {summary.new_files > 0 && (
                  <li className="flex items-center gap-2">
                    <span className="text-[#636366]">&bull;</span>
                    {summary.new_files} new audio file{summary.new_files !== 1 ? "s" : ""}
                    {summary.total_bytes_uploaded > 0 && (
                      <span className="text-[#98989d]">
                        ({formatBytes(summary.total_bytes_uploaded)})
                      </span>
                    )}
                  </li>
                )}
                {summary.modified_files > 0 && (
                  <li className="flex items-center gap-2">
                    <span className="text-[#636366]">&bull;</span>
                    {summary.modified_files} modified file{summary.modified_files !== 1 ? "s" : ""}
                  </li>
                )}
                <li className="flex items-center gap-2">
                  <span className="text-[#636366]">&bull;</span>
                  Session file modified
                </li>
                {summary.auto_pushes > 0 && (
                  <li className="flex items-center gap-2">
                    <span className="text-[#636366]">&bull;</span>
                    {summary.auto_pushes} auto-push{summary.auto_pushes !== 1 ? "es" : ""} over{" "}
                    {summary.duration_minutes < 60
                      ? `${summary.duration_minutes} min`
                      : `${Math.round(summary.duration_minutes / 60)} hours`}
                  </li>
                )}
              </ul>
            ) : (
              <div className="text-xs text-[#636366]">
                Could not load change summary
              </div>
            )}
          </div>

          {/* Release note */}
          <div>
            <label
              htmlFor="release-note"
              className="block text-xs text-[#98989d] mb-1"
            >
              Add a note (optional):
            </label>
            <textarea
              id="release-note"
              value={note}
              onChange={(e) => setNote(e.target.value)}
              placeholder="rough mix v2, added bass ODs"
              rows={2}
              className="w-full bg-[#1c1c1e] border border-[#3a3a3c] rounded-md px-3 py-2 text-xs text-[#f5f5f7] placeholder-[#636366] resize-none focus:outline-none focus:border-[#0a84ff]"
            />
          </div>

          {/* Error */}
          {error && (
            <div className="text-xs text-[#ff453a] bg-[#3a1a18] rounded px-2 py-1.5">
              {error}
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="flex items-center justify-end gap-2 px-4 py-3 border-t border-[#3a3a3c]">
          <button
            onClick={onCancel}
            disabled={releasing}
            className="px-3 py-1.5 text-xs text-[#98989d] hover:text-[#f5f5f7] rounded-md hover:bg-[#3a3a3c] transition-colors disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            onClick={handleRelease}
            disabled={releasing}
            className="px-3 py-1.5 text-xs font-medium text-white bg-[#0a84ff] hover:bg-[#0a84ff]/80 rounded-md transition-colors disabled:opacity-50 flex items-center gap-1.5"
          >
            {releasing && (
              <span className="w-3 h-3 border border-white/30 border-t-white rounded-full animate-spin" />
            )}
            Release
          </button>
        </div>
      </div>
    </div>
  );
}
