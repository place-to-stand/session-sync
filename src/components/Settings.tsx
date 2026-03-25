import { useState, useEffect, useCallback } from "react";
import { useQuery } from "convex/react";
import { api } from "../lib/convex";
import {
  getConfig,
  getIgnorePatterns,
  saveIgnorePatterns,
  getLogLines,
  exportLogs,
  getAppVersion,
  type AppConfig,
} from "../lib/commands";

interface SettingsProps {
  onClose: () => void;
}

type SettingsTab = "general" | "ignore" | "logs";

export default function Settings({ onClose }: SettingsProps) {
  const [activeTab, setActiveTab] = useState<SettingsTab>("general");
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [appVersion, setAppVersion] = useState<string>("");
  const [ignorePatterns, setIgnorePatterns] = useState<string>("");
  const [ignoreModified, setIgnoreModified] = useState(false);
  const [ignoreSaving, setIgnoreSaving] = useState(false);
  const [logLines, setLogLines] = useState<string[]>([]);
  const [logLoading, setLogLoading] = useState(false);
  const [exportPath, setExportPath] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Convex connection status
  const sessions = useQuery(api.sessions.listSessions, {
    includeArchived: false,
  });
  const isConvexConnected = sessions !== undefined;

  // Load config and version on mount
  useEffect(() => {
    async function load() {
      try {
        const [cfg, version] = await Promise.all([
          getConfig(),
          getAppVersion(),
        ]);
        setConfig(cfg);
        setAppVersion(version);
      } catch {
        setError("Failed to load configuration");
      }
    }
    load();
  }, []);

  // Load ignore patterns when tab changes
  useEffect(() => {
    if (activeTab === "ignore") {
      getIgnorePatterns()
        .then(setIgnorePatterns)
        .catch(() => setIgnorePatterns(""));
    }
  }, [activeTab]);

  // Load logs when tab changes
  useEffect(() => {
    if (activeTab === "logs") {
      setLogLoading(true);
      getLogLines(100)
        .then(setLogLines)
        .catch(() => setLogLines(["Failed to load logs"]))
        .finally(() => setLogLoading(false));
    }
  }, [activeTab]);

  const handleSaveIgnorePatterns = useCallback(async () => {
    setIgnoreSaving(true);
    try {
      await saveIgnorePatterns(ignorePatterns);
      setIgnoreModified(false);
    } catch {
      setError("Failed to save ignore patterns");
    }
    setIgnoreSaving(false);
  }, [ignorePatterns]);

  const handleExportLogs = useCallback(async () => {
    try {
      const path = await exportLogs();
      setExportPath(path);
    } catch {
      setError("Failed to export logs");
    }
  }, []);

  const handleRefreshLogs = useCallback(async () => {
    setLogLoading(true);
    try {
      const lines = await getLogLines(100);
      setLogLines(lines);
    } catch {
      setLogLines(["Failed to load logs"]);
    }
    setLogLoading(false);
  }, []);

  const tabs: Array<{ id: SettingsTab; label: string }> = [
    { id: "general", label: "General" },
    { id: "ignore", label: "Ignore" },
    { id: "logs", label: "Logs" },
  ];

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
        <span className="text-xs font-semibold text-[#f5f5f7]">Settings</span>
        <div className="w-8" />
      </div>

      {/* Tab bar */}
      <div className="flex border-b border-[#3a3a3c]">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            onClick={() => setActiveTab(tab.id)}
            className={`flex-1 py-2 text-[11px] font-medium transition-colors ${
              activeTab === tab.id
                ? "text-[#0a84ff] border-b-2 border-[#0a84ff]"
                : "text-[#98989d] hover:text-[#f5f5f7]"
            }`}
          >
            {tab.label}
          </button>
        ))}
      </div>

      {/* Error banner */}
      {error && (
        <div className="px-3 py-2 text-xs text-[#ff453a] bg-[#3a1a18] border-b border-[#ff453a]/20">
          {error}
          <button
            onClick={() => setError(null)}
            className="ml-2 text-[#98989d]"
          >
            dismiss
          </button>
        </div>
      )}

      {/* Tab content */}
      <div className="flex-1 overflow-y-auto">
        {/* General tab */}
        {activeTab === "general" && (
          <div className="p-3 space-y-4">
            {/* Machine info */}
            <section>
              <div className="text-[10px] font-semibold text-[#98989d] uppercase tracking-wider mb-2">
                Machine
              </div>
              <div className="bg-[#2c2c2e] rounded-lg divide-y divide-[#3a3a3c]">
                <div className="flex items-center justify-between px-3 py-2">
                  <span className="text-xs text-[#98989d]">Name</span>
                  <span className="text-xs text-[#f5f5f7]">
                    {config?.machine_name ?? "--"}
                  </span>
                </div>
                <div className="flex items-center justify-between px-3 py-2">
                  <span className="text-xs text-[#98989d]">User</span>
                  <span className="text-xs text-[#f5f5f7]">
                    {config?.user_name ?? "--"}
                  </span>
                </div>
                <div className="flex items-center justify-between px-3 py-2">
                  <span className="text-xs text-[#98989d]">Machine ID</span>
                  <span className="text-[10px] text-[#636366] font-mono truncate max-w-[180px]">
                    {config?.machine_id ?? "--"}
                  </span>
                </div>
                <div className="flex items-center justify-between px-3 py-2">
                  <span className="text-xs text-[#98989d]">Role</span>
                  <span className="text-xs text-[#f5f5f7]">
                    {config?.is_admin ? "Admin" : "Engineer"}
                  </span>
                </div>
              </div>
            </section>

            {/* Connection status */}
            <section>
              <div className="text-[10px] font-semibold text-[#98989d] uppercase tracking-wider mb-2">
                Connection
              </div>
              <div className="bg-[#2c2c2e] rounded-lg divide-y divide-[#3a3a3c]">
                <div className="flex items-center justify-between px-3 py-2">
                  <span className="text-xs text-[#98989d]">Convex</span>
                  <div className="flex items-center gap-1.5">
                    <span
                      className={`status-dot ${
                        isConvexConnected
                          ? "status-dot-green"
                          : "status-dot-red"
                      }`}
                    />
                    <span className="text-xs text-[#f5f5f7]">
                      {isConvexConnected ? "Connected" : "Disconnected"}
                    </span>
                  </div>
                </div>
              </div>
            </section>

            {/* App version */}
            <section>
              <div className="text-[10px] font-semibold text-[#98989d] uppercase tracking-wider mb-2">
                About
              </div>
              <div className="bg-[#2c2c2e] rounded-lg px-3 py-2">
                <div className="flex items-center justify-between">
                  <span className="text-xs text-[#98989d]">Version</span>
                  <span className="text-xs text-[#636366]">
                    {appVersion || "--"}
                  </span>
                </div>
              </div>
            </section>
          </div>
        )}

        {/* Ignore patterns tab */}
        {activeTab === "ignore" && (
          <div className="p-3 space-y-3">
            <div className="text-[10px] text-[#98989d]">
              Files matching these patterns will not be synced. Uses gitignore
              syntax. One pattern per line.
            </div>
            <textarea
              value={ignorePatterns}
              onChange={(e) => {
                setIgnorePatterns(e.target.value);
                setIgnoreModified(true);
              }}
              placeholder={`# Example ignore patterns
*.bak
*.tmp
.DS_Store
Thumbs.db
Session File Backups/`}
              rows={14}
              className="w-full bg-[#2c2c2e] border border-[#3a3a3c] rounded-lg px-3 py-2 text-xs text-[#f5f5f7] font-mono placeholder-[#636366] resize-none focus:outline-none focus:border-[#0a84ff]"
              spellCheck={false}
            />
            <div className="flex items-center justify-between">
              <span className="text-[10px] text-[#636366]">
                {ignoreModified ? "Unsaved changes" : ""}
              </span>
              <button
                onClick={handleSaveIgnorePatterns}
                disabled={!ignoreModified || ignoreSaving}
                className="px-3 py-1.5 text-[11px] font-medium bg-[#0a84ff] text-white rounded hover:bg-[#0a84ff]/80 transition-colors disabled:opacity-30"
              >
                {ignoreSaving ? "Saving..." : "Save"}
              </button>
            </div>
          </div>
        )}

        {/* Logs tab */}
        {activeTab === "logs" && (
          <div className="p-3 space-y-3">
            <div className="flex items-center justify-between">
              <span className="text-[10px] text-[#98989d]">
                Last 100 log entries
              </span>
              <div className="flex items-center gap-2">
                <button
                  onClick={handleRefreshLogs}
                  disabled={logLoading}
                  className="text-[10px] text-[#0a84ff] hover:text-[#0a84ff]/80 disabled:opacity-50"
                >
                  Refresh
                </button>
                <button
                  onClick={handleExportLogs}
                  className="text-[10px] text-[#0a84ff] hover:text-[#0a84ff]/80"
                >
                  Export
                </button>
              </div>
            </div>

            {exportPath && (
              <div className="text-[10px] text-[#30d158] bg-[#1a2a1a] rounded px-2 py-1">
                Exported to: {exportPath}
              </div>
            )}

            <div className="bg-[#2c2c2e] rounded-lg border border-[#3a3a3c] overflow-hidden">
              {logLoading ? (
                <div className="flex items-center justify-center py-8">
                  <span className="text-[10px] text-[#636366]">
                    Loading logs...
                  </span>
                </div>
              ) : (
                <div className="max-h-[350px] overflow-y-auto p-2">
                  {logLines.length === 0 ? (
                    <div className="text-[10px] text-[#636366] text-center py-4">
                      No log entries
                    </div>
                  ) : (
                    <pre className="text-[10px] text-[#98989d] font-mono leading-relaxed whitespace-pre-wrap break-all">
                      {logLines.join("\n")}
                    </pre>
                  )}
                </div>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
