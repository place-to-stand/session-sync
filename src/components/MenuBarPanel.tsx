import { useState, useEffect, useCallback } from "react";
import { useQuery } from "convex/react";
import { api } from "../lib/convex";
import { open } from "@tauri-apps/plugin-dialog";
import type { SyncProgressPayload, PullProgressPayload } from "../hooks/useTauriEvents";
import type { SyncProgressMap, PullProgressMap } from "../App";
import { addSession, watchDirectory, getConfig } from "../lib/commands";
import SessionCard from "./SessionCard";
import ActivityFeed from "./ActivityFeed";
import Settings from "./Settings";
import VersionHistory from "./VersionHistory";

interface MenuBarPanelProps {
  syncProgress: SyncProgressMap;
  pullProgress: PullProgressMap;
}

type PanelView = "main" | "settings" | "versions";

/**
 * Main menu bar panel. This is the primary UI shown in the macOS tray popup.
 *
 * Layout:
 *   - Header: "SessionSync" title + gear icon
 *   - ACTIVE SESSIONS section with SessionCard list
 *   - ACTIVITY section with recent events
 *   - Footer: Archived count, Add Session, Settings
 */
export default function MenuBarPanel({
  syncProgress,
  pullProgress,
}: MenuBarPanelProps) {
  const [view, setView] = useState<PanelView>("main");
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(
    null,
  );
  const [currentMachineId, setCurrentMachineId] = useState<string | null>(null);
  const [addingSession, setAddingSession] = useState(false);

  // Real-time session list from Convex
  const sessions = useQuery(api.sessions.listSessions, {
    includeArchived: false,
  });
  const allSessions = useQuery(api.sessions.listSessions, {
    includeArchived: true,
  });

  // Load machine ID on mount
  useEffect(() => {
    getConfig()
      .then((cfg) => setCurrentMachineId(cfg.machine_id))
      .catch(() => {});
  }, []);

  const activeSessions = sessions ?? [];
  const archivedCount = allSessions
    ? allSessions.filter((s) => s.status === "archived").length
    : 0;

  const handleShowVersions = useCallback((sessionId: string) => {
    setSelectedSessionId(sessionId);
    setView("versions");
  }, []);

  async function handleAddSession() {
    setAddingSession(true);
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Choose a Pro Tools session folder",
      });
      if (selected && typeof selected === "string") {
        await addSession(selected);
      }
    } catch {
      // User cancelled or error
    }
    setAddingSession(false);
  }

  // Settings panel
  if (view === "settings") {
    return <Settings onClose={() => setView("main")} />;
  }

  // Version history panel
  if (view === "versions" && selectedSessionId) {
    return (
      <VersionHistory
        sessionId={selectedSessionId}
        onClose={() => {
          setView("main");
          setSelectedSessionId(null);
        }}
      />
    );
  }

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2.5 border-b border-[#3a3a3c] shrink-0">
        <h1 className="text-sm font-semibold text-[#f5f5f7]">SessionSync</h1>
        <button
          onClick={() => setView("settings")}
          className="text-[#98989d] hover:text-[#f5f5f7] transition-colors p-1 rounded hover:bg-[#3a3a3c]"
          title="Settings"
        >
          <svg
            className="w-4 h-4"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
            strokeWidth={1.5}
          >
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              d="M9.594 3.94c.09-.542.56-.94 1.11-.94h2.593c.55 0 1.02.398 1.11.94l.213 1.281c.063.374.313.686.645.87.074.04.147.083.22.127.325.196.72.257 1.075.124l1.217-.456a1.125 1.125 0 0 1 1.37.49l1.296 2.247a1.125 1.125 0 0 1-.26 1.431l-1.003.827c-.293.241-.438.613-.43.992a7.723 7.723 0 0 1 0 .255c-.008.378.137.75.43.991l1.004.827c.424.35.534.955.26 1.43l-1.298 2.247a1.125 1.125 0 0 1-1.369.491l-1.217-.456c-.355-.133-.75-.072-1.076.124a6.47 6.47 0 0 1-.22.128c-.331.183-.581.495-.644.869l-.213 1.281c-.09.543-.56.94-1.11.94h-2.594c-.55 0-1.019-.398-1.11-.94l-.213-1.281c-.062-.374-.312-.686-.644-.87a6.52 6.52 0 0 1-.22-.127c-.325-.196-.72-.257-1.076-.124l-1.217.456a1.125 1.125 0 0 1-1.369-.49l-1.297-2.247a1.125 1.125 0 0 1 .26-1.431l1.004-.827c.292-.24.437-.613.43-.991a6.932 6.932 0 0 1 0-.255c.007-.38-.138-.751-.43-.992l-1.004-.827a1.125 1.125 0 0 1-.26-1.43l1.297-2.247a1.125 1.125 0 0 1 1.37-.491l1.216.456c.356.133.751.072 1.076-.124.072-.044.146-.086.22-.128.332-.183.582-.495.644-.869l.214-1.28Z"
            />
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              d="M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0Z"
            />
          </svg>
        </button>
      </div>

      {/* Scrollable content */}
      <div className="flex-1 overflow-y-auto min-h-0">
        {/* ACTIVE SESSIONS */}
        <div className="pt-2">
          <div className="px-3 pb-1.5">
            <span className="text-[10px] font-semibold text-[#98989d] uppercase tracking-wider">
              Active Sessions
            </span>
          </div>

          {sessions === undefined ? (
            <div className="flex items-center justify-center py-8">
              <div className="flex flex-col items-center gap-2">
                <div className="w-4 h-4 border-2 border-[#0a84ff] border-t-transparent rounded-full animate-spin" />
                <span className="text-[10px] text-[#636366]">
                  Connecting to Convex...
                </span>
              </div>
            </div>
          ) : activeSessions.length === 0 ? (
            <div className="flex flex-col items-center py-8 px-6">
              <span className="text-2xl mb-2">{"\uD83C\uDFB5"}</span>
              <span className="text-xs text-[#98989d] text-center">
                No active sessions. Add a session or watch a folder to get
                started.
              </span>
            </div>
          ) : (
            <div className="divide-y divide-[#2c2c2e]">
              {activeSessions.map((session) => (
                <SessionCard
                  key={session._id}
                  session={session}
                  currentMachineId={currentMachineId}
                  syncStatus={syncProgress[session._id] ?? null}
                  pullProgress={pullProgress[session._id] ?? null}
                  onShowVersions={handleShowVersions}
                />
              ))}
            </div>
          )}
        </div>

        {/* Divider */}
        <div className="border-t border-[#3a3a3c] mt-1" />

        {/* ACTIVITY */}
        <div className="pt-2 pb-1">
          <div className="px-3 pb-1.5">
            <span className="text-[10px] font-semibold text-[#98989d] uppercase tracking-wider">
              Activity
            </span>
          </div>
          <ActivityFeed limit={5} />
        </div>
      </div>

      {/* Footer */}
      <div className="border-t border-[#3a3a3c] shrink-0 divide-y divide-[#2c2c2e]">
        {/* Archived count */}
        {archivedCount > 0 && (
          <button className="flex items-center justify-between w-full px-3 py-2 hover:bg-[#2c2c2e] transition-colors">
            <div className="flex items-center gap-2 text-xs text-[#98989d]">
              <span>{"\uD83D\uDCE6"}</span>
              <span>Archived ({archivedCount})</span>
            </div>
            <svg
              className="w-3 h-3 text-[#636366]"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={2}
            >
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                d="M8.25 4.5l7.5 7.5-7.5 7.5"
              />
            </svg>
          </button>
        )}

        {/* Add Session */}
        <button
          onClick={handleAddSession}
          disabled={addingSession}
          className="flex items-center justify-between w-full px-3 py-2 hover:bg-[#2c2c2e] transition-colors disabled:opacity-50"
        >
          <div className="flex items-center gap-2 text-xs text-[#98989d]">
            <span className="text-[#0a84ff] font-semibold">+</span>
            <span>Add Session</span>
          </div>
          <svg
            className="w-3 h-3 text-[#636366]"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
            strokeWidth={2}
          >
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              d="M8.25 4.5l7.5 7.5-7.5 7.5"
            />
          </svg>
        </button>

        {/* Settings */}
        <button
          onClick={() => setView("settings")}
          className="flex items-center justify-between w-full px-3 py-2 hover:bg-[#2c2c2e] transition-colors"
        >
          <div className="flex items-center gap-2 text-xs text-[#98989d]">
            <span>{"\u2699"}</span>
            <span>Settings</span>
          </div>
          <svg
            className="w-3 h-3 text-[#636366]"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
            strokeWidth={2}
          >
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              d="M8.25 4.5l7.5 7.5-7.5 7.5"
            />
          </svg>
        </button>
      </div>
    </div>
  );
}
