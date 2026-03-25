import { api } from "../../../convex/_generated/api";

/**
 * Re-export the typed Convex API for use throughout the frontend.
 * Components should import { api } from "@/lib/convex" rather than
 * reaching into convex/_generated directly.
 */
export { api };

/**
 * Format a timestamp as relative time ("just now", "3 min ago", "2 hours ago", "yesterday").
 */
export function formatRelativeTime(timestamp: number): string {
  const now = Date.now();
  const diffMs = now - timestamp;

  if (diffMs < 0) {
    return "just now";
  }

  const seconds = Math.floor(diffMs / 1000);
  const minutes = Math.floor(seconds / 60);
  const hours = Math.floor(minutes / 60);
  const days = Math.floor(hours / 24);

  if (seconds < 60) {
    return "just now";
  }
  if (minutes === 1) {
    return "1 min ago";
  }
  if (minutes < 60) {
    return `${minutes} min ago`;
  }
  if (hours === 1) {
    return "1 hour ago";
  }
  if (hours < 24) {
    return `${hours} hours ago`;
  }
  if (days === 1) {
    return "yesterday";
  }
  if (days < 7) {
    return `${days} days ago`;
  }

  // Fall back to a short date
  const date = new Date(timestamp);
  return date.toLocaleDateString("en-US", { month: "short", day: "numeric" });
}

/**
 * Format a byte count as a human-readable size ("14.2 GB", "500 MB", "120 KB").
 */
export function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";

  const units = ["B", "KB", "MB", "GB", "TB"];
  const k = 1024;
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  const value = bytes / Math.pow(k, i);

  // Use 1 decimal for GB+, 0 for smaller units
  if (i >= 3) {
    return `${value.toFixed(1)} ${units[i]}`;
  }
  if (i >= 2) {
    return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[i]}`;
  }
  return `${Math.round(value)} ${units[i]}`;
}

/**
 * Format seconds as a human-readable ETA ("~8 min remaining", "~2 hours remaining").
 */
export function formatEta(seconds: number): string {
  if (seconds <= 0) return "finishing up";

  const minutes = Math.ceil(seconds / 60);
  const hours = Math.floor(minutes / 60);

  if (minutes < 2) {
    return "~1 min remaining";
  }
  if (minutes < 60) {
    return `~${minutes} min remaining`;
  }
  if (hours === 1) {
    const remainingMin = minutes - 60;
    if (remainingMin > 0) {
      return `~1 hr ${remainingMin} min remaining`;
    }
    return "~1 hour remaining";
  }
  return `~${hours} hours remaining`;
}

/**
 * Format a timestamp as a short time string ("2:30 PM").
 */
export function formatTime(timestamp: number): string {
  const date = new Date(timestamp);
  return date.toLocaleTimeString("en-US", {
    hour: "numeric",
    minute: "2-digit",
    hour12: true,
  });
}
