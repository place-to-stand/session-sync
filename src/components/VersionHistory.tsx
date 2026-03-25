import { useQuery } from "convex/react";
import { api } from "../lib/convex";
import { formatRelativeTime, formatBytes } from "../lib/convex";
import { rollbackSession, pullSession } from "../lib/commands";
import { useState } from "react";
import type { Id } from "../../convex/_generated/dataModel";
import { useMachines } from "../hooks/useConvex";

interface VersionHistoryProps {
  sessionId: string;
  onClose: () => void;
}

/**
 * Displays the version history for a session.
 * Each version shows its number, who pushed it, when, file count,
 * and a "Pull This Version" button.
 */
export default function VersionHistory({
  sessionId,
  onClose,
}: VersionHistoryProps) {
  const [rollingBack, setRollingBack] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  const session = useQuery(api.sessions.getSession, {
    sessionId: sessionId as Id<"sessions">,
  });

  const versions = useQuery(api.versions.listVersions, {
    sessionId: sessionId as Id<"sessions">,
  });

  // Resolve machine names for pushedBy fields
  const machines = useMachines();

  // Build a lookup map for machine display names
  const machineMap = new Map<string, string>();
  if (machines) {
    for (const m of machines) {
      machineMap.set(m._id, m.displayName);
    }
  }

  async function handlePullVersion(versionNumber: number) {
    setRollingBack(versionNumber);
    setError(null);
    try {
      await rollbackSession(sessionId, versionNumber);
      setRollingBack(null);
    } catch (err) {
      setError(
        err instanceof Error ? err.message : "Failed to pull version",
      );
      setRollingBack(null);
    }
  }

  return (
    <div className="flex flex-col h-full bg-[#1c1c1e]">
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2.5 border-b border-[#3a3a3c]">
        <button
          onClick={onClose}
          className="text-xs text-[#0a84ff] hover:text-[#0a84ff]/80"
        >
          Back
        </button>
        <span className="text-xs font-semibold text-[#f5f5f7]">
          {session?.name ?? "Session"} — Versions
        </span>
        <div className="w-8" /> {/* Spacer for centering */}
      </div>

      {/* Error */}
      {error && (
        <div className="px-3 py-2 text-xs text-[#ff453a] bg-[#3a1a18] border-b border-[#ff453a]/20">
          {error}
        </div>
      )}

      {/* Version list */}
      <div className="flex-1 overflow-y-auto">
        {versions === undefined ? (
          <div className="flex items-center justify-center py-12">
            <div className="flex flex-col items-center gap-2">
              <div className="w-4 h-4 border-2 border-[#0a84ff] border-t-transparent rounded-full animate-spin" />
              <span className="text-xs text-[#636366]">
                Loading version history...
              </span>
            </div>
          </div>
        ) : versions.length === 0 ? (
          <div className="flex items-center justify-center py-12">
            <span className="text-xs text-[#636366]">
              No versions yet
            </span>
          </div>
        ) : (
          <div className="divide-y divide-[#2c2c2e]">
            {versions.map((version) => {
              const pushedByName =
                machineMap.get(version.pushedBy) ?? "Unknown";

              return (
                <div
                  key={version.versionNumber}
                  className="px-3 py-2.5 hover:bg-[#2c2c2e]/50 transition-colors"
                >
                  <div className="flex items-start justify-between gap-2">
                    {/* Version info */}
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-2">
                        <span className="text-xs font-medium text-[#f5f5f7]">
                          v{version.versionNumber}
                        </span>
                        {version.isRelease && (
                          <span className="px-1.5 py-0.5 text-[10px] font-medium bg-[#30d158]/20 text-[#30d158] rounded">
                            release
                          </span>
                        )}
                        <span className="text-[10px] text-[#636366]">
                          {formatRelativeTime(version.createdAt)}
                        </span>
                      </div>

                      <div className="mt-0.5 text-[11px] text-[#98989d]">
                        {version.autoSummary}
                      </div>

                      {version.releaseNote && (
                        <div className="mt-0.5 text-[11px] text-[#f5f5f7] italic">
                          &ldquo;{version.releaseNote}&rdquo;
                        </div>
                      )}

                      <div className="mt-0.5 text-[10px] text-[#636366]">
                        {version.filesChanged} files changed &middot;{" "}
                        {formatBytes(version.bytesChanged)} &middot;{" "}
                        {pushedByName}
                      </div>
                    </div>

                    {/* Pull this version button */}
                    <button
                      onClick={() => handlePullVersion(version.versionNumber)}
                      disabled={rollingBack !== null}
                      className="shrink-0 px-2 py-1 text-[10px] text-[#0a84ff] hover:bg-[#0a84ff]/10 rounded transition-colors disabled:opacity-50"
                    >
                      {rollingBack === version.versionNumber
                        ? "Pulling..."
                        : "Pull This Version"}
                    </button>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
