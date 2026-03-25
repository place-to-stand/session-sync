import { useState, useRef, useEffect } from "react";
import type { Doc } from "../../convex/_generated/dataModel";
import type { SyncProgressPayload, PullProgressPayload } from "../hooks/useTauriEvents";
import { formatRelativeTime } from "../lib/convex";
import {
  checkoutSession,
  releaseSession,
  claimSession,
  pullSession,
  requestSession,
} from "../lib/commands";
import SyncBacklog from "./SyncBacklog";
import PullProgress from "./PullProgress";
import ReleaseDialog from "./ReleaseDialog";
import { useMachine } from "../hooks/useConvex";

type Session = Doc<"sessions">;

interface SessionCardProps {
  session: Session;
  /** Machine ID of the current user's machine */
  currentMachineId: string | null;
  syncStatus: SyncProgressPayload | null;
  pullProgress: PullProgressPayload | null;
  onShowVersions: (sessionId: string) => void;
}

type SessionVisualState =
  | "available"
  | "updates_available"
  | "checked_out_by_you"
  | "checked_out_by_other"
  | "stale"
  | "pulling"
  | "drive_disconnected"
  | "archived";

const stateIcons: Record<SessionVisualState, string> = {
  available: "\u26AA",
  updates_available: "\uD83D\uDD35",
  checked_out_by_you: "\uD83D\uDFE2",
  checked_out_by_other: "\uD83D\uDFE1",
  stale: "\uD83D\uDFE0",
  pulling: "\u23F3",
  drive_disconnected: "\u26A0\uFE0F",
  archived: "\uD83D\uDCE6",
};

function getVisualState(
  session: Session,
  currentMachineId: string | null,
  isPulling: boolean,
): SessionVisualState {
  if (isPulling) return "pulling";
  if (session.status === "archived") return "archived";
  if (session.status === "stale") return "stale";
  if (session.status === "checked_out") {
    if (session.checkedOutBy === currentMachineId) {
      return "checked_out_by_you";
    }
    return "checked_out_by_other";
  }
  // "available" status -- could be "updates_available" if there is a new release
  // For MVP, we treat all available as the base available state
  return "available";
}

function getStatusText(
  session: Session,
  visualState: SessionVisualState,
  holderName: string | null,
): string {
  switch (visualState) {
    case "checked_out_by_you":
      return "Checked out by you";
    case "checked_out_by_other":
      return `Checked out by ${holderName ?? "another machine"}`;
    case "stale":
      return `Stale -- ${holderName ?? "machine"} offline`;
    case "pulling":
      return "Pulling...";
    case "drive_disconnected":
      return "Drive disconnected";
    case "archived":
      return "Archived";
    case "updates_available":
      return "New release available";
    case "available":
    default:
      return "Available";
  }
}

export default function SessionCard({
  session,
  currentMachineId,
  syncStatus,
  pullProgress,
  onShowVersions,
}: SessionCardProps) {
  const [showDropdown, setShowDropdown] = useState(false);
  const [showReleaseDialog, setShowReleaseDialog] = useState(false);
  const [actionLoading, setActionLoading] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const dropdownRef = useRef<HTMLDivElement>(null);

  // Resolve the checkout holder's display name
  const holderMachine = useMachine(
    session.checkedOutBy ?? undefined,
  );
  const holderName = holderMachine?.displayName ?? null;

  const isPulling = pullProgress != null;
  const visualState = getVisualState(session, currentMachineId, isPulling);
  const statusText = getStatusText(session, visualState, holderName);
  const icon = stateIcons[visualState];

  // Close dropdown on click outside
  useEffect(() => {
    function handleClickOutside(e: MouseEvent) {
      if (
        dropdownRef.current &&
        !dropdownRef.current.contains(e.target as Node)
      ) {
        setShowDropdown(false);
      }
    }
    if (showDropdown) {
      document.addEventListener("mousedown", handleClickOutside);
    }
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [showDropdown]);

  // Clear errors after 5 seconds
  useEffect(() => {
    if (actionError) {
      const t = setTimeout(() => setActionError(null), 5000);
      return () => clearTimeout(t);
    }
  }, [actionError]);

  async function runAction(name: string, fn: () => Promise<void>) {
    setActionLoading(name);
    setActionError(null);
    setShowDropdown(false);
    try {
      await fn();
    } catch (err) {
      setActionError(err instanceof Error ? err.message : `Failed: ${name}`);
    }
    setActionLoading(null);
  }

  // Render the action button(s) based on state
  function renderActions() {
    if (visualState === "drive_disconnected" || visualState === "archived") {
      return (
        <span className="text-[10px] text-[#636366]">--</span>
      );
    }

    if (visualState === "pulling") {
      return null; // Pull progress component handles cancel
    }

    if (visualState === "checked_out_by_you") {
      return (
        <button
          onClick={() => setShowReleaseDialog(true)}
          disabled={actionLoading !== null}
          className="px-2.5 py-1 text-[11px] font-medium bg-[#30d158] text-black rounded hover:bg-[#30d158]/80 transition-colors disabled:opacity-50"
        >
          {actionLoading === "release" ? "..." : "Release"}
        </button>
      );
    }

    if (visualState === "stale") {
      return (
        <button
          onClick={() =>
            runAction("claim", () => claimSession(session._id))
          }
          disabled={actionLoading !== null}
          className="px-2.5 py-1 text-[11px] font-medium bg-[#ff9f0a] text-black rounded hover:bg-[#ff9f0a]/80 transition-colors disabled:opacity-50"
        >
          {actionLoading === "claim" ? "..." : "Claim"}
        </button>
      );
    }

    // Available or checked_out_by_other -- show Pull dropdown
    return (
      <div className="relative" ref={dropdownRef}>
        <button
          onClick={() => setShowDropdown(!showDropdown)}
          disabled={actionLoading !== null}
          className="flex items-center gap-1 px-2.5 py-1 text-[11px] font-medium bg-[#3a3a3c] text-[#f5f5f7] rounded hover:bg-[#48484a] transition-colors disabled:opacity-50"
        >
          {actionLoading ? "..." : "Pull"}
          <svg
            className="w-2.5 h-2.5"
            fill="none"
            viewBox="0 0 10 6"
            stroke="currentColor"
            strokeWidth={1.5}
          >
            <path d="M1 1l4 4 4-4" />
          </svg>
        </button>

        {showDropdown && (
          <div className="dropdown-enter absolute right-0 top-full mt-1 w-48 bg-[#2c2c2e] border border-[#3a3a3c] rounded-lg shadow-xl overflow-hidden z-10">
            {visualState !== "checked_out_by_other" && (
              <button
                onClick={() =>
                  runAction("pull-released", () =>
                    pullSession(session._id, "released"),
                  )
                }
                className="w-full text-left px-3 py-2 text-xs text-[#f5f5f7] hover:bg-[#3a3a3c] transition-colors"
              >
                Pull Released Version
              </button>
            )}
            <button
              onClick={() =>
                runAction("pull-latest", () =>
                  pullSession(session._id, "latest"),
                )
              }
              className="w-full text-left px-3 py-2 text-xs text-[#f5f5f7] hover:bg-[#3a3a3c] transition-colors"
            >
              Pull Latest (Prov.)
            </button>
            {visualState === "available" || visualState === "updates_available" ? (
              <button
                onClick={() =>
                  runAction("checkout", () => checkoutSession(session._id))
                }
                className="w-full text-left px-3 py-2 text-xs text-[#0a84ff] hover:bg-[#3a3a3c] transition-colors border-t border-[#3a3a3c]"
              >
                Check Out
              </button>
            ) : visualState === "checked_out_by_other" ? (
              <button
                onClick={() =>
                  runAction("request", () => requestSession(session._id))
                }
                className="w-full text-left px-3 py-2 text-xs text-[#ff9f0a] hover:bg-[#3a3a3c] transition-colors border-t border-[#3a3a3c]"
              >
                Request
              </button>
            ) : null}
          </div>
        )}
      </div>
    );
  }

  return (
    <>
      <div className="px-3 py-2 hover:bg-[#2c2c2e]/50 transition-colors">
        {/* Main row */}
        <div className="flex items-start justify-between gap-2">
          {/* Left: icon + info */}
          <div className="flex items-start gap-2 min-w-0 flex-1">
            <span className="text-sm mt-0.5 shrink-0" role="img" aria-label={visualState}>
              {icon}
            </span>
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-1.5">
                <button
                  onClick={() => onShowVersions(session._id)}
                  className="text-xs font-medium text-[#f5f5f7] hover:text-[#0a84ff] truncate transition-colors"
                  title={`${session.name} — click for version history`}
                >
                  {session.name}
                </button>
                {holderName && visualState === "checked_out_by_other" && (
                  <span className="text-[10px] text-[#636366] shrink-0">
                    ({holderName})
                  </span>
                )}
              </div>
              <div className="text-[11px] text-[#98989d] mt-0.5">
                {statusText}
              </div>
            </div>
          </div>

          {/* Right: action button */}
          <div className="shrink-0 mt-0.5">{renderActions()}</div>
        </div>

        {/* Sync backlog (when checked out by you) */}
        {visualState === "checked_out_by_you" && (
          <div className="mt-1.5 pl-6">
            <SyncBacklog syncStatus={syncStatus} />
          </div>
        )}

        {/* Pull progress */}
        {isPulling && pullProgress && (
          <div className="mt-2 pl-6">
            <PullProgress
              sessionId={session._id}
              sessionName={session.name}
              progress={pullProgress}
            />
          </div>
        )}

        {/* Last saved timestamp */}
        {session.updatedAt && (
          <div className="mt-1 pl-6 text-[10px] text-[#636366]">
            Last updated {formatRelativeTime(session.updatedAt)}
          </div>
        )}

        {/* Action error */}
        {actionError && (
          <div className="mt-1.5 pl-6 text-[10px] text-[#ff453a]">
            {actionError}
          </div>
        )}
      </div>

      {/* Release dialog */}
      {showReleaseDialog && (
        <ReleaseDialog
          sessionId={session._id}
          sessionName={session.name}
          onRelease={() => setShowReleaseDialog(false)}
          onCancel={() => setShowReleaseDialog(false)}
        />
      )}
    </>
  );
}
