import { invoke } from "@tauri-apps/api/core";

// ---- Types ----

export interface SyncStatus {
  session_id: string;
  state: "synced" | "pushing" | "queued";
  files_pending: number;
  bytes_pending: number;
  current_file: string | null;
}

export interface AppConfig {
  machine_id: string;
  machine_name: string;
  user_name: string;
  convex_url: string;
  is_admin: boolean;
  setup_complete: boolean;
}

export interface R2TestResult {
  success: boolean;
  error: string | null;
  bucket_accessible: boolean;
}

export interface InviteResult {
  invite_url: string;
  token: string;
  expires_at: number;
}

export interface RedeemResult {
  success: boolean;
  user_id: string;
  machine_id: string;
}

export interface SessionSummary {
  new_files: number;
  modified_files: number;
  total_bytes_uploaded: number;
  auto_pushes: number;
  duration_minutes: number;
}

// ---- Session Actions ----

/**
 * Check out a session. Acquires exclusive lock via Convex.
 */
export async function checkoutSession(sessionId: string): Promise<void> {
  await invoke("checkout_session", { sessionId });
}

/**
 * Release a checked-out session with optional note.
 * Creates a release version snapshot before unlocking.
 */
export async function releaseSession(
  sessionId: string,
  note?: string,
): Promise<void> {
  await invoke("release_session", { sessionId, note: note ?? null });
}

/**
 * Claim a stale session. Only works when the session heartbeat has expired.
 */
export async function claimSession(sessionId: string): Promise<void> {
  await invoke("claim_session", { sessionId });
}

/**
 * Pull a session from R2. Mode determines which version to download.
 * - "released": last explicitly released version (canonical)
 * - "latest": most recent auto-pushed snapshot (provisional)
 */
export async function pullSession(
  sessionId: string,
  mode: "released" | "latest",
): Promise<void> {
  await invoke("pull_session", { sessionId, mode });
}

/**
 * Cancel an in-progress pull. Downloaded files are kept.
 */
export async function cancelPull(sessionId: string): Promise<void> {
  await invoke("cancel_pull", { sessionId });
}

/**
 * Request a session that is checked out by another engineer.
 * Sends an advisory notification to the holder.
 */
export async function requestSession(sessionId: string): Promise<void> {
  await invoke("request_session", { sessionId });
}

// ---- Session Management ----

/**
 * Add a specific session folder to be tracked.
 */
export async function addSession(path: string): Promise<void> {
  await invoke("add_session", { path });
}

/**
 * Watch a directory for Pro Tools sessions. Auto-detects .ptx files.
 */
export async function watchDirectory(path: string): Promise<void> {
  await invoke("watch_directory", { path });
}

/**
 * Archive a session. Stops syncing but preserves data in R2.
 */
export async function archiveSession(sessionId: string): Promise<void> {
  await invoke("archive_session", { sessionId });
}

/**
 * Unarchive a previously archived session.
 */
export async function unarchiveSession(sessionId: string): Promise<void> {
  await invoke("unarchive_session", { sessionId });
}

/**
 * Rollback a session to a specific version number.
 */
export async function rollbackSession(
  sessionId: string,
  versionNumber: number,
): Promise<void> {
  await invoke("rollback_session", { sessionId, versionNumber });
}

// ---- Sync Status ----

/**
 * Get the current sync status for all active sessions.
 */
export async function getSyncStatus(): Promise<SyncStatus[]> {
  return await invoke<SyncStatus[]>("get_sync_status");
}

/**
 * Get the release summary for a session (changes since checkout).
 */
export async function getReleaseSummary(
  sessionId: string,
): Promise<SessionSummary> {
  return await invoke<SessionSummary>("get_release_summary", { sessionId });
}

// ---- Configuration ----

/**
 * Get the current app configuration.
 */
export async function getConfig(): Promise<AppConfig> {
  return await invoke<AppConfig>("get_config");
}

/**
 * Save setup configuration (admin flow).
 */
export async function saveSetupConfig(config: {
  user_name: string;
  machine_name: string;
  r2_account_id: string;
  r2_access_key: string;
  r2_secret_key: string;
  r2_bucket: string;
}): Promise<void> {
  await invoke("save_setup_config", config);
}

/**
 * Test R2 connection with provided credentials.
 */
export async function testR2Connection(credentials: {
  account_id: string;
  access_key: string;
  secret_key: string;
  bucket: string;
}): Promise<R2TestResult> {
  return await invoke<R2TestResult>("test_r2_connection", credentials);
}

/**
 * Create an invite link for a partner engineer.
 */
export async function createInvite(): Promise<InviteResult> {
  return await invoke<InviteResult>("create_invite");
}

/**
 * Redeem an invite link (invited engineer flow).
 */
export async function redeemInvite(
  inviteUrl: string,
  userName: string,
  machineName: string,
): Promise<RedeemResult> {
  return await invoke<RedeemResult>("redeem_invite", {
    inviteUrl,
    userName,
    machineName,
  });
}

/**
 * Mark setup as complete. Transitions from wizard to main panel.
 */
export async function completeSetup(): Promise<void> {
  await invoke("complete_setup");
}

// ---- Ignore Patterns ----

/**
 * Get the current ignore patterns (gitignore-style).
 */
export async function getIgnorePatterns(): Promise<string> {
  return await invoke<string>("get_ignore_patterns");
}

/**
 * Save updated ignore patterns.
 */
export async function saveIgnorePatterns(patterns: string): Promise<void> {
  await invoke("save_ignore_patterns", { patterns });
}

// ---- Logs ----

/**
 * Get the last N lines of the app log.
 */
export async function getLogLines(count: number): Promise<string[]> {
  return await invoke<string[]>("get_log_lines", { count });
}

/**
 * Export logs to a file and return the file path.
 */
export async function exportLogs(): Promise<string> {
  return await invoke<string>("export_logs");
}

// ---- System ----

/**
 * Get the hostname of this machine (for pre-filling setup).
 */
export async function getHostname(): Promise<string> {
  return await invoke<string>("get_hostname");
}

/**
 * Get the app version string.
 */
export async function getAppVersion(): Promise<string> {
  return await invoke<string>("get_app_version");
}
