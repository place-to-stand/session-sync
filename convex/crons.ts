import { cronJobs } from "convex/server";
import { internal } from "./_generated/api";

const crons = cronJobs();

// Run stale session detection every 5 minutes instead of scheduling
// from each heartbeat (which creates unbounded duplicate jobs).
crons.interval(
  "detect stale sessions",
  { minutes: 5 },
  internal.machines.detectStaleSessions,
);

export default crons;
