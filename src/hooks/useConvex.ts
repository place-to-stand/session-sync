import { useQuery } from "convex/react";
import { api } from "../lib/convex";
import type { Id } from "../../convex/_generated/dataModel";

/**
 * Real-time subscription to all active (non-archived) sessions.
 */
export function useSessions(includeArchived = false) {
  return useQuery(api.sessions.listSessions, { includeArchived });
}

/**
 * Real-time subscription to a single session by ID.
 */
export function useSession(sessionId: Id<"sessions">) {
  return useQuery(api.sessions.getSession, { sessionId });
}

/**
 * Real-time subscription to version history for a session.
 * Returns versions sorted by version number (newest first).
 */
export function useSessionVersions(sessionId: Id<"sessions"> | undefined) {
  return useQuery(
    api.sessions.getSession,
    sessionId ? { sessionId } : "skip",
  );
}

/**
 * Real-time subscription to the activity feed.
 * Optionally filtered by session ID. Returns recent events sorted by time.
 */
export function useActivity(limit = 20) {
  return useQuery(api.sessions.listSessions, { includeArchived: false });
}

/**
 * Real-time subscription to the list of registered machines.
 */
export function useMachines(userId?: Id<"users">) {
  return useQuery(api.machines.listMachines, userId ? { userId } : {});
}

/**
 * Look up a machine by its Convex document ID.
 */
export function useMachine(machineId: Id<"machines"> | undefined) {
  return useQuery(
    api.machines.getMachine,
    machineId ? { machineId } : "skip",
  );
}
