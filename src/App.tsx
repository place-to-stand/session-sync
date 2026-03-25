import { useState, useEffect, useCallback } from "react";
import { useTauriEvents } from "./hooks/useTauriEvents";
import type {
  SyncProgressPayload,
  PullProgressPayload,
  SyncErrorPayload,
  NewReleasePayload,
  SessionRequestedPayload,
  SessionStateChangedPayload,
} from "./hooks/useTauriEvents";
import { getConfig, type AppConfig } from "./lib/commands";
import SetupWizard from "./components/SetupWizard";
import MenuBarPanel from "./components/MenuBarPanel";

export type PullProgressMap = Record<string, PullProgressPayload>;
export type SyncProgressMap = Record<string, SyncProgressPayload>;

export default function App() {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Sync and pull progress tracked per session
  const [syncProgress, setSyncProgress] = useState<SyncProgressMap>({});
  const [pullProgress, setPullProgress] = useState<PullProgressMap>({});
  const [syncErrors, setSyncErrors] = useState<SyncErrorPayload[]>([]);
  const [notifications, setNotifications] = useState<string[]>([]);

  // Load config on mount to determine if setup is complete
  useEffect(() => {
    let cancelled = false;

    async function loadConfig() {
      try {
        const cfg = await getConfig();
        if (!cancelled) {
          setConfig(cfg);
          setLoading(false);
        }
      } catch {
        // Config not found means setup hasn't been done
        if (!cancelled) {
          setConfig(null);
          setLoading(false);
        }
      }
    }

    loadConfig();
    return () => {
      cancelled = true;
    };
  }, []);

  // Listen for Tauri events from the Rust backend
  const handleSyncProgress = useCallback((payload: SyncProgressPayload) => {
    setSyncProgress((prev) => ({
      ...prev,
      [payload.session_id]: payload,
    }));
  }, []);

  const handlePullProgress = useCallback((payload: PullProgressPayload) => {
    setPullProgress((prev) => ({
      ...prev,
      [payload.session_id]: payload,
    }));
    // Clear pull progress when complete
    if (payload.files_completed >= payload.files_total && payload.files_total > 0) {
      setTimeout(() => {
        setPullProgress((prev) => {
          const next = { ...prev };
          delete next[payload.session_id];
          return next;
        });
      }, 2000);
    }
  }, []);

  const handleSyncError = useCallback((payload: SyncErrorPayload) => {
    setSyncErrors((prev) => [...prev.slice(-9), payload]);
  }, []);

  const handleNewRelease = useCallback((payload: NewReleasePayload) => {
    setNotifications((prev) => [
      ...prev.slice(-4),
      `${payload.released_by} released ${payload.session_name}`,
    ]);
  }, []);

  const handleSessionRequested = useCallback(
    (payload: SessionRequestedPayload) => {
      setNotifications((prev) => [
        ...prev.slice(-4),
        `${payload.machine_name} is requesting ${payload.session_name}`,
      ]);
    },
    [],
  );

  const handleSessionStateChanged = useCallback(
    (_payload: SessionStateChangedPayload) => {
      // State changes are handled via Convex real-time subscriptions.
      // This event is used for triggering UI refreshes if needed.
    },
    [],
  );

  useTauriEvents({
    onSyncProgress: handleSyncProgress,
    onPullProgress: handlePullProgress,
    onSyncError: handleSyncError,
    onNewRelease: handleNewRelease,
    onSessionRequested: handleSessionRequested,
    onSessionStateChanged: handleSessionStateChanged,
  });

  const handleSetupComplete = useCallback(async () => {
    try {
      const cfg = await getConfig();
      setConfig(cfg);
    } catch {
      // Fallback: mark as configured anyway
      setConfig({
        machine_id: "",
        machine_name: "",
        user_name: "",
        convex_url: "",
        is_admin: false,
        setup_complete: true,
      });
    }
  }, []);

  const dismissError = useCallback((index: number) => {
    setSyncErrors((prev) => prev.filter((_, i) => i !== index));
  }, []);

  const dismissNotification = useCallback((index: number) => {
    setNotifications((prev) => prev.filter((_, i) => i !== index));
  }, []);

  if (loading) {
    return (
      <div className="flex items-center justify-center h-screen bg-[#1c1c1e]">
        <div className="flex flex-col items-center gap-3">
          <div className="w-6 h-6 border-2 border-[#0a84ff] border-t-transparent rounded-full animate-spin" />
          <span className="text-[#98989d] text-xs">Loading...</span>
        </div>
      </div>
    );
  }

  if (!config?.setup_complete) {
    return <SetupWizard onComplete={handleSetupComplete} />;
  }

  return (
    <div className="flex flex-col h-screen bg-[#1c1c1e]">
      {/* Error banners */}
      {syncErrors.map((err, i) => (
        <div
          key={`error-${i}`}
          className="flex items-center gap-2 px-3 py-2 bg-[#3a1a18] border-b border-[#ff453a]/30 text-xs"
        >
          <span className="text-[#ff453a] flex-1 truncate">
            Sync error: {err.error}
          </span>
          <button
            onClick={() => dismissError(i)}
            className="text-[#98989d] hover:text-[#f5f5f7] shrink-0"
          >
            x
          </button>
        </div>
      ))}

      {/* Notification banners */}
      {notifications.map((note, i) => (
        <div
          key={`note-${i}`}
          className="flex items-center gap-2 px-3 py-2 bg-[#1a2a3a] border-b border-[#0a84ff]/30 text-xs"
        >
          <span className="text-[#0a84ff] flex-1 truncate">{note}</span>
          <button
            onClick={() => dismissNotification(i)}
            className="text-[#98989d] hover:text-[#f5f5f7] shrink-0"
          >
            x
          </button>
        </div>
      ))}

      <MenuBarPanel
        syncProgress={syncProgress}
        pullProgress={pullProgress}
      />
    </div>
  );
}
