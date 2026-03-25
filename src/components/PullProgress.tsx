import { useState } from "react";
import type { PullProgressPayload } from "../hooks/useTauriEvents";
import { formatBytes, formatEta } from "../lib/convex";
import { cancelPull } from "../lib/commands";

interface PullProgressProps {
  sessionId: string;
  sessionName: string;
  progress: PullProgressPayload;
}

/**
 * Displays download progress for a session pull operation.
 * Shows overall progress, ETA, current file, and cancel option.
 */
export default function PullProgress({
  sessionId,
  sessionName,
  progress,
}: PullProgressProps) {
  const [confirmingCancel, setConfirmingCancel] = useState(false);
  const [cancelling, setCancelling] = useState(false);

  const {
    files_total,
    files_completed,
    bytes_total,
    bytes_completed,
    current_file,
    eta_seconds,
  } = progress;

  const percentage =
    bytes_total > 0 ? Math.round((bytes_completed / bytes_total) * 100) : 0;

  const fileName = current_file?.split("/").pop() ?? null;

  async function handleCancel() {
    if (!confirmingCancel) {
      setConfirmingCancel(true);
      return;
    }
    setCancelling(true);
    try {
      await cancelPull(sessionId);
    } catch {
      // Already cancelled or completed
    }
    setCancelling(false);
    setConfirmingCancel(false);
  }

  return (
    <div className="bg-[#2c2c2e] rounded-lg border border-[#3a3a3c] p-3 space-y-2">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="text-xs font-medium text-[#f5f5f7]">
          Pulling {sessionName}
        </div>
        <div className="text-[10px] text-[#98989d]">
          {percentage}%
        </div>
      </div>

      {/* Progress bar */}
      <div className="progress-bar-track">
        <div
          className="progress-bar-fill"
          style={{ width: `${percentage}%` }}
        />
      </div>

      {/* Stats row */}
      <div className="flex items-center justify-between text-[10px]">
        <span className="text-[#98989d]">
          {formatBytes(bytes_completed)} / {formatBytes(bytes_total)}
        </span>
        {eta_seconds != null && eta_seconds > 0 && (
          <span className="text-[#98989d]">{formatEta(eta_seconds)}</span>
        )}
      </div>

      {/* Current file */}
      {fileName && (
        <div className="text-[10px] text-[#636366] truncate">
          Downloading: {fileName}
        </div>
      )}

      {/* File count */}
      <div className="text-[10px] text-[#636366]">
        {files_completed} / {files_total} files
      </div>

      {/* Cancel */}
      <div className="flex justify-end pt-1">
        {confirmingCancel ? (
          <div className="flex items-center gap-2">
            <span className="text-[10px] text-[#ff9f0a]">
              Stop pull? Downloaded files will be kept.
            </span>
            <button
              onClick={() => setConfirmingCancel(false)}
              disabled={cancelling}
              className="text-[10px] text-[#98989d] hover:text-[#f5f5f7]"
            >
              No
            </button>
            <button
              onClick={handleCancel}
              disabled={cancelling}
              className="text-[10px] text-[#ff453a] hover:text-[#ff453a]/80"
            >
              {cancelling ? "Cancelling..." : "Yes, stop"}
            </button>
          </div>
        ) : (
          <button
            onClick={handleCancel}
            className="text-[10px] text-[#98989d] hover:text-[#ff453a] transition-colors"
          >
            Cancel
          </button>
        )}
      </div>
    </div>
  );
}
