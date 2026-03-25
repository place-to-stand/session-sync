import type { SyncProgressPayload } from "../hooks/useTauriEvents";
import { formatBytes } from "../lib/convex";

interface SyncBacklogProps {
  syncStatus: SyncProgressPayload | null;
}

/**
 * Displays the current sync backlog status for a checked-out session.
 * Three visual states:
 *  - Synced (green dot)
 *  - Pushing N files (blue animated dot)
 *  - N GB queued (yellow dot)
 */
export default function SyncBacklog({ syncStatus }: SyncBacklogProps) {
  if (!syncStatus) {
    return (
      <div className="flex items-center gap-1.5">
        <span className="status-dot status-dot-green" />
        <span className="text-[#98989d] text-xs">Synced</span>
      </div>
    );
  }

  const {
    files_total,
    files_completed,
    bytes_total,
    bytes_completed,
    current_file,
  } = syncStatus;

  const filesRemaining = files_total - files_completed;
  const bytesRemaining = bytes_total - bytes_completed;

  // All files pushed -- synced
  if (filesRemaining <= 0) {
    return (
      <div className="flex items-center gap-1.5">
        <span className="status-dot status-dot-green" />
        <span className="text-[#98989d] text-xs">Synced</span>
      </div>
    );
  }

  // Actively pushing (there is a current file)
  if (current_file) {
    const fileName = current_file.split("/").pop() ?? current_file;
    return (
      <div className="flex flex-col gap-0.5">
        <div className="flex items-center gap-1.5">
          <span className="status-dot status-dot-blue" />
          <span className="text-[#0a84ff] text-xs">
            Pushing {filesRemaining} file{filesRemaining !== 1 ? "s" : ""}
            {bytesRemaining > 0 && (
              <span className="text-[#98989d]">
                {" "}({formatBytes(bytesRemaining)} remaining)
              </span>
            )}
          </span>
        </div>
        <span className="text-[#636366] text-[10px] pl-[14px] truncate">
          {fileName}
        </span>
      </div>
    );
  }

  // Queued but not yet actively pushing
  return (
    <div className="flex items-center gap-1.5">
      <span className="status-dot status-dot-yellow" />
      <span className="text-[#ffd60a] text-xs">
        {formatBytes(bytesRemaining)} queued
      </span>
    </div>
  );
}
