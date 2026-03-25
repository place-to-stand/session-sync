import { useQuery } from "convex/react";
import { api } from "../lib/convex";
import { formatRelativeTime } from "../lib/convex";
import type { Doc, Id } from "../../convex/_generated/dataModel";

interface ActivityFeedProps {
  limit?: number;
}

type ActivityAction =
  | "checkout"
  | "release"
  | "pull"
  | "push"
  | "archive"
  | "unarchive"
  | "request"
  | "claim";

const actionColors: Record<ActivityAction, string> = {
  checkout: "text-[#ffd60a]",
  release: "text-[#30d158]",
  pull: "text-[#0a84ff]",
  push: "text-[#98989d]",
  archive: "text-[#636366]",
  unarchive: "text-[#636366]",
  request: "text-[#ff9f0a]",
  claim: "text-[#ff9f0a]",
};

const actionVerbs: Record<ActivityAction, string> = {
  checkout: "checked out",
  release: "released",
  pull: "pulled",
  push: "pushed",
  archive: "archived",
  unarchive: "unarchived",
  request: "requested",
  claim: "claimed",
};

/**
 * Displays a list of recent activity events.
 * Events are streamed from Convex in real-time.
 */
export default function ActivityFeed({ limit = 5 }: ActivityFeedProps) {
  // We need sessions and machines to resolve names.
  // In a real app these would be joined server-side; for MVP we query separately.
  const sessions = useQuery(api.sessions.listSessions, {
    includeArchived: true,
  });
  const machines = useQuery(api.machines.listMachines, {});

  // Build lookup maps
  const sessionMap = new Map<string, string>();
  if (sessions) {
    for (const s of sessions) {
      sessionMap.set(s._id, s.name);
    }
  }

  const machineMap = new Map<string, string>();
  if (machines) {
    for (const m of machines) {
      machineMap.set(m._id, m.displayName);
    }
  }

  // For the MVP, we don't have a dedicated activity query yet.
  // We display placeholder entries derived from session data.
  // In production, this would be: useQuery(api.activity.listRecent, { limit })

  if (!sessions || !machines) {
    return (
      <div className="px-3 py-2">
        <div className="text-[#636366] text-xs">Loading activity...</div>
      </div>
    );
  }

  // Derive recent activity from session state as a proxy
  // In production, this would come from the activity table
  const recentActivity = sessions
    .filter((s) => s.updatedAt)
    .sort((a, b) => b.updatedAt - a.updatedAt)
    .slice(0, limit)
    .map((session) => {
      const action: ActivityAction =
        session.status === "checked_out"
          ? "checkout"
          : session.status === "archived"
            ? "archive"
            : "release";

      const machineName = session.checkedOutBy
        ? machineMap.get(session.checkedOutBy) ?? "Unknown"
        : "You";

      return {
        id: session._id,
        machineName,
        action,
        sessionName: session.name,
        timestamp: session.updatedAt,
      };
    });

  if (recentActivity.length === 0) {
    return (
      <div className="px-3 py-2">
        <div className="text-[#636366] text-xs">No recent activity</div>
      </div>
    );
  }

  return (
    <div className="flex flex-col">
      {recentActivity.map((event) => (
        <div
          key={event.id}
          className="flex items-baseline gap-1 px-3 py-1 text-xs leading-relaxed"
        >
          <span className={actionColors[event.action]}>
            {event.machineName}
          </span>
          <span className="text-[#98989d]">
            {actionVerbs[event.action]}
          </span>
          <span className="text-[#f5f5f7] truncate flex-1">
            {event.sessionName}
          </span>
          <span className="text-[#636366] shrink-0 ml-1">
            {formatRelativeTime(event.timestamp)}
          </span>
        </div>
      ))}
    </div>
  );
}
