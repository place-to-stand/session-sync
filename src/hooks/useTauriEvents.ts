import { useEffect, useRef } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * Generic hook to listen to a single Tauri event.
 * Automatically subscribes on mount and unsubscribes on unmount.
 */
export function useTauriEvent<T>(
  eventName: string,
  handler: (payload: T) => void,
): void {
  const handlerRef = useRef(handler);
  handlerRef.current = handler;

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;

    listen<T>(eventName, (event) => {
      if (!cancelled) {
        handlerRef.current(event.payload);
      }
    }).then((fn) => {
      if (cancelled) {
        fn();
      } else {
        unlisten = fn;
      }
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [eventName]);
}

// ---- Typed event payloads ----

export interface SyncProgressPayload {
  session_id: string;
  files_total: number;
  files_completed: number;
  bytes_total: number;
  bytes_completed: number;
  current_file: string | null;
}

export interface SessionStateChangedPayload {
  session_id: string;
  old_status: string;
  new_status: string;
  checked_out_by: string | null;
}

export interface PullProgressPayload {
  session_id: string;
  files_total: number;
  files_completed: number;
  bytes_total: number;
  bytes_completed: number;
  current_file: string | null;
  eta_seconds: number | null;
}

export interface SyncErrorPayload {
  session_id: string;
  error: string;
  recoverable: boolean;
}

export interface NewReleasePayload {
  session_id: string;
  session_name: string;
  released_by: string;
  version_number: number;
  release_note: string | null;
  auto_summary: string;
}

export interface SessionRequestedPayload {
  session_id: string;
  session_name: string;
  requested_by: string;
  machine_name: string;
}

export interface StaleCheckoutPayload {
  session_id: string;
  session_name: string;
  checked_out_by: string;
  last_heartbeat: number;
}

/**
 * Convenience hook that sets up all Tauri event listeners at once.
 * Pass handlers for the events you care about.
 */
export function useTauriEvents(handlers: {
  onSyncProgress?: (payload: SyncProgressPayload) => void;
  onSessionStateChanged?: (payload: SessionStateChangedPayload) => void;
  onPullProgress?: (payload: PullProgressPayload) => void;
  onSyncError?: (payload: SyncErrorPayload) => void;
  onNewRelease?: (payload: NewReleasePayload) => void;
  onSessionRequested?: (payload: SessionRequestedPayload) => void;
  onStaleCheckout?: (payload: StaleCheckoutPayload) => void;
}): void {
  const handlersRef = useRef(handlers);
  handlersRef.current = handlers;

  useEffect(() => {
    const unlisteners: UnlistenFn[] = [];
    let cancelled = false;

    const events: Array<{ name: string; key: keyof typeof handlers }> = [
      { name: "sync-progress", key: "onSyncProgress" },
      { name: "session-state-changed", key: "onSessionStateChanged" },
      { name: "pull-progress", key: "onPullProgress" },
      { name: "sync-error", key: "onSyncError" },
      { name: "new-release", key: "onNewRelease" },
      { name: "session-requested", key: "onSessionRequested" },
      { name: "stale-checkout", key: "onStaleCheckout" },
    ];

    for (const { name, key } of events) {
      listen(name, (event) => {
        if (!cancelled) {
          const fn = handlersRef.current[key];
          if (fn) {
            (fn as (payload: unknown) => void)(event.payload);
          }
        }
      }).then((unlisten) => {
        if (cancelled) {
          unlisten();
        } else {
          unlisteners.push(unlisten);
        }
      });
    }

    return () => {
      cancelled = true;
      for (const unlisten of unlisteners) {
        unlisten();
      }
    };
  }, []);
}
