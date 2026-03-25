import { useState, useEffect } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  getHostname,
  saveSetupConfig,
  testR2Connection,
  redeemInvite,
  createInvite,
  addSession,
  watchDirectory,
  completeSetup,
  type R2TestResult,
  type InviteResult,
} from "../lib/commands";

interface SetupWizardProps {
  onComplete: () => void;
}

type SetupFlow = "admin" | "invited" | null;
type Step = 1 | 2 | 3 | 4 | 5;

/**
 * Multi-step setup wizard shown on first launch.
 *
 * Admin flow (5 steps):
 *   1. Welcome / choose flow
 *   2. Identity (name, machine name)
 *   3. R2 credentials + test connection
 *   4. Add first session (watch folder / add specific / skip)
 *   5. Invite partner (generate link + copy)
 *
 * Invited flow (4 steps):
 *   1. Welcome / choose flow
 *   2. Identity
 *   3. Paste invite link + connect
 *   4. Choose session folder
 */
export default function SetupWizard({ onComplete }: SetupWizardProps) {
  const [step, setStep] = useState<Step>(1);
  const [flow, setFlow] = useState<SetupFlow>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  // Step 2: Identity
  const [userName, setUserName] = useState("");
  const [machineName, setMachineName] = useState("");

  // Step 3a: Admin R2 credentials
  const [accountId, setAccountId] = useState("");
  const [accessKey, setAccessKey] = useState("");
  const [secretKey, setSecretKey] = useState("");
  const [bucket, setBucket] = useState("session-sync");
  const [r2Tested, setR2Tested] = useState(false);
  const [r2Testing, setR2Testing] = useState(false);

  // Step 3b: Invited flow
  const [inviteLink, setInviteLink] = useState("");

  // Step 4: First session
  const [sessionPath, setSessionPath] = useState<string | null>(null);

  // Step 5: Invite partner
  const [inviteResult, setInviteResult] = useState<InviteResult | null>(null);
  const [copied, setCopied] = useState(false);

  // Pre-fill hostname
  useEffect(() => {
    let cancelled = false;
    getHostname()
      .then((hostname) => {
        if (!cancelled && !machineName) {
          setMachineName(hostname);
        }
      })
      .catch(() => {
        // Hostname unavailable, user fills manually
      });
    return () => {
      cancelled = true;
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  function clearError() {
    setError(null);
  }

  async function handleFinish() {
    setLoading(true);
    setError(null);
    try {
      await completeSetup();
      onComplete();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to complete setup");
      setLoading(false);
    }
  }

  // ---- Step 1: Welcome ----
  function renderStep1() {
    return (
      <div className="flex flex-col items-center text-center px-6 py-8">
        <div className="text-3xl mb-4">{"\uD83C\uDFB5"}</div>
        <h1 className="text-lg font-semibold text-[#f5f5f7] mb-2">
          SessionSync
        </h1>
        <p className="text-xs text-[#98989d] leading-relaxed mb-8 max-w-[280px]">
          Keep your Pro Tools sessions in sync across studios. Explicit
          checkout, automatic background sync, zero conflicts.
        </p>

        <div className="space-y-2 w-full max-w-[260px]">
          <button
            onClick={() => {
              setFlow("admin");
              setStep(2);
            }}
            className="w-full py-2.5 text-sm font-medium bg-[#0a84ff] text-white rounded-lg hover:bg-[#0a84ff]/80 transition-colors"
          >
            Set Up New Workspace
          </button>
          <button
            onClick={() => {
              setFlow("invited");
              setStep(2);
            }}
            className="w-full py-2.5 text-sm font-medium bg-[#3a3a3c] text-[#f5f5f7] rounded-lg hover:bg-[#48484a] transition-colors"
          >
            I Have an Invite Link
          </button>
        </div>
      </div>
    );
  }

  // ---- Step 2: Identity ----
  function renderStep2() {
    const canAdvance =
      userName.trim().length > 0 && machineName.trim().length > 0;

    return (
      <div className="px-6 py-6">
        <h2 className="text-sm font-semibold text-[#f5f5f7] mb-1">
          Your Identity
        </h2>
        <p className="text-xs text-[#98989d] mb-5">
          This identifies you and your machine to collaborators.
        </p>

        <div className="space-y-4">
          <div>
            <label
              htmlFor="user-name"
              className="block text-xs text-[#98989d] mb-1"
            >
              Your Name
            </label>
            <input
              id="user-name"
              type="text"
              value={userName}
              onChange={(e) => setUserName(e.target.value)}
              placeholder="Jason Desiderio"
              className="w-full bg-[#2c2c2e] border border-[#3a3a3c] rounded-md px-3 py-2 text-xs text-[#f5f5f7] placeholder-[#636366] focus:outline-none focus:border-[#0a84ff]"
              autoFocus
            />
          </div>

          <div>
            <label
              htmlFor="machine-name"
              className="block text-xs text-[#98989d] mb-1"
            >
              Machine Name
            </label>
            <input
              id="machine-name"
              type="text"
              value={machineName}
              onChange={(e) => setMachineName(e.target.value)}
              placeholder="Austin Studio"
              className="w-full bg-[#2c2c2e] border border-[#3a3a3c] rounded-md px-3 py-2 text-xs text-[#f5f5f7] placeholder-[#636366] focus:outline-none focus:border-[#0a84ff]"
            />
            <div className="text-[10px] text-[#636366] mt-1">
              Pre-filled from hostname. Change to something recognizable.
            </div>
          </div>
        </div>

        <div className="flex justify-between mt-6">
          <button
            onClick={() => setStep(1)}
            className="px-3 py-1.5 text-xs text-[#98989d] hover:text-[#f5f5f7] transition-colors"
          >
            Back
          </button>
          <button
            onClick={() => setStep(3)}
            disabled={!canAdvance}
            className="px-4 py-1.5 text-xs font-medium bg-[#0a84ff] text-white rounded-md hover:bg-[#0a84ff]/80 transition-colors disabled:opacity-30"
          >
            Next
          </button>
        </div>
      </div>
    );
  }

  // ---- Step 3a: Admin -- R2 Credentials ----
  function renderStep3Admin() {
    const canTest =
      accountId.trim().length > 0 &&
      accessKey.trim().length > 0 &&
      secretKey.trim().length > 0 &&
      bucket.trim().length > 0;

    async function handleTestConnection() {
      setR2Testing(true);
      setError(null);
      try {
        const result: R2TestResult = await testR2Connection({
          account_id: accountId.trim(),
          access_key: accessKey.trim(),
          secret_key: secretKey.trim(),
          bucket: bucket.trim(),
        });
        if (result.success) {
          setR2Tested(true);
        } else {
          setError(result.error ?? "Connection test failed");
        }
      } catch (err) {
        setError(
          err instanceof Error ? err.message : "Connection test failed",
        );
      }
      setR2Testing(false);
    }

    async function handleNext() {
      setLoading(true);
      setError(null);
      try {
        await saveSetupConfig({
          user_name: userName.trim(),
          machine_name: machineName.trim(),
          r2_account_id: accountId.trim(),
          r2_access_key: accessKey.trim(),
          r2_secret_key: secretKey.trim(),
          r2_bucket: bucket.trim(),
        });
        setStep(4);
      } catch (err) {
        setError(
          err instanceof Error ? err.message : "Failed to save config",
        );
      }
      setLoading(false);
    }

    return (
      <div className="px-6 py-6">
        <h2 className="text-sm font-semibold text-[#f5f5f7] mb-1">
          Connect Storage
        </h2>
        <p className="text-xs text-[#98989d] mb-5">
          SessionSync uses Cloudflare R2 for file storage. Enter your R2 API
          credentials below.
        </p>

        <div className="space-y-3">
          <div>
            <label className="block text-xs text-[#98989d] mb-1">
              Account ID
            </label>
            <input
              type="text"
              value={accountId}
              onChange={(e) => {
                setAccountId(e.target.value);
                setR2Tested(false);
              }}
              placeholder="a1b2c3d4e5f6..."
              className="w-full bg-[#2c2c2e] border border-[#3a3a3c] rounded-md px-3 py-2 text-xs text-[#f5f5f7] placeholder-[#636366] font-mono focus:outline-none focus:border-[#0a84ff]"
            />
          </div>

          <div>
            <label className="block text-xs text-[#98989d] mb-1">
              Access Key ID
            </label>
            <input
              type="text"
              value={accessKey}
              onChange={(e) => {
                setAccessKey(e.target.value);
                setR2Tested(false);
              }}
              placeholder="AKIA..."
              className="w-full bg-[#2c2c2e] border border-[#3a3a3c] rounded-md px-3 py-2 text-xs text-[#f5f5f7] placeholder-[#636366] font-mono focus:outline-none focus:border-[#0a84ff]"
            />
          </div>

          <div>
            <label className="block text-xs text-[#98989d] mb-1">
              Secret Access Key
            </label>
            <input
              type="password"
              value={secretKey}
              onChange={(e) => {
                setSecretKey(e.target.value);
                setR2Tested(false);
              }}
              placeholder="wJalrXUtnF..."
              className="w-full bg-[#2c2c2e] border border-[#3a3a3c] rounded-md px-3 py-2 text-xs text-[#f5f5f7] placeholder-[#636366] font-mono focus:outline-none focus:border-[#0a84ff]"
            />
          </div>

          <div>
            <label className="block text-xs text-[#98989d] mb-1">
              Bucket Name
            </label>
            <input
              type="text"
              value={bucket}
              onChange={(e) => {
                setBucket(e.target.value);
                setR2Tested(false);
              }}
              className="w-full bg-[#2c2c2e] border border-[#3a3a3c] rounded-md px-3 py-2 text-xs text-[#f5f5f7] placeholder-[#636366] font-mono focus:outline-none focus:border-[#0a84ff]"
            />
          </div>

          {/* Test button */}
          <button
            onClick={handleTestConnection}
            disabled={!canTest || r2Testing}
            className={`w-full py-2 text-xs font-medium rounded-md transition-colors ${
              r2Tested
                ? "bg-[#30d158]/20 text-[#30d158] border border-[#30d158]/30"
                : "bg-[#3a3a3c] text-[#f5f5f7] hover:bg-[#48484a]"
            } disabled:opacity-30`}
          >
            {r2Testing
              ? "Testing..."
              : r2Tested
                ? "Connection Successful"
                : "Test Connection"}
          </button>
        </div>

        {error && (
          <div className="mt-3 text-xs text-[#ff453a] bg-[#3a1a18] rounded px-2 py-1.5">
            {error}
          </div>
        )}

        <div className="flex justify-between mt-5">
          <button
            onClick={() => {
              setStep(2);
              clearError();
            }}
            className="px-3 py-1.5 text-xs text-[#98989d] hover:text-[#f5f5f7] transition-colors"
          >
            Back
          </button>
          <button
            onClick={handleNext}
            disabled={!r2Tested || loading}
            className="px-4 py-1.5 text-xs font-medium bg-[#0a84ff] text-white rounded-md hover:bg-[#0a84ff]/80 transition-colors disabled:opacity-30 flex items-center gap-1.5"
          >
            {loading && (
              <span className="w-3 h-3 border border-white/30 border-t-white rounded-full animate-spin" />
            )}
            Next
          </button>
        </div>
      </div>
    );
  }

  // ---- Step 3b: Invited -- Paste Invite Link ----
  function renderStep3Invited() {
    async function handleConnect() {
      setLoading(true);
      setError(null);
      try {
        await redeemInvite(
          inviteLink.trim(),
          userName.trim(),
          machineName.trim(),
        );
        setStep(4);
      } catch (err) {
        setError(
          err instanceof Error ? err.message : "Failed to redeem invite",
        );
      }
      setLoading(false);
    }

    return (
      <div className="px-6 py-6">
        <h2 className="text-sm font-semibold text-[#f5f5f7] mb-1">
          Connect with Invite
        </h2>
        <p className="text-xs text-[#98989d] mb-5">
          Paste the invite link your partner shared with you. You will not need
          any storage credentials.
        </p>

        <div>
          <label className="block text-xs text-[#98989d] mb-1">
            Invite Link
          </label>
          <input
            type="text"
            value={inviteLink}
            onChange={(e) => setInviteLink(e.target.value)}
            placeholder="https://sessionsync.convex.site/invite/abc123..."
            className="w-full bg-[#2c2c2e] border border-[#3a3a3c] rounded-md px-3 py-2 text-xs text-[#f5f5f7] placeholder-[#636366] font-mono focus:outline-none focus:border-[#0a84ff]"
            autoFocus
          />
        </div>

        {error && (
          <div className="mt-3 text-xs text-[#ff453a] bg-[#3a1a18] rounded px-2 py-1.5">
            {error}
          </div>
        )}

        <div className="flex justify-between mt-6">
          <button
            onClick={() => {
              setStep(2);
              clearError();
            }}
            className="px-3 py-1.5 text-xs text-[#98989d] hover:text-[#f5f5f7] transition-colors"
          >
            Back
          </button>
          <button
            onClick={handleConnect}
            disabled={!inviteLink.trim() || loading}
            className="px-4 py-1.5 text-xs font-medium bg-[#0a84ff] text-white rounded-md hover:bg-[#0a84ff]/80 transition-colors disabled:opacity-30 flex items-center gap-1.5"
          >
            {loading && (
              <span className="w-3 h-3 border border-white/30 border-t-white rounded-full animate-spin" />
            )}
            Connect
          </button>
        </div>
      </div>
    );
  }

  // ---- Step 4: Add First Session ----
  function renderStep4() {
    async function handleWatchFolder() {
      setError(null);
      try {
        const selected = await open({
          directory: true,
          multiple: false,
          title: "Choose a folder to watch for Pro Tools sessions",
        });
        if (selected && typeof selected === "string") {
          setLoading(true);
          await watchDirectory(selected);
          setSessionPath(selected);
          setLoading(false);
        }
      } catch (err) {
        setError(
          err instanceof Error ? err.message : "Failed to watch folder",
        );
        setLoading(false);
      }
    }

    async function handleAddSession() {
      setError(null);
      try {
        const selected = await open({
          directory: true,
          multiple: false,
          title: "Choose a Pro Tools session folder",
        });
        if (selected && typeof selected === "string") {
          setLoading(true);
          await addSession(selected);
          setSessionPath(selected);
          setLoading(false);
        }
      } catch (err) {
        setError(
          err instanceof Error ? err.message : "Failed to add session",
        );
        setLoading(false);
      }
    }

    function handleNext() {
      if (flow === "admin") {
        setStep(5);
      } else {
        handleFinish();
      }
    }

    return (
      <div className="px-6 py-6">
        <h2 className="text-sm font-semibold text-[#f5f5f7] mb-1">
          {flow === "invited"
            ? "Choose Session Folder"
            : "Add Your First Session"}
        </h2>
        <p className="text-xs text-[#98989d] mb-5">
          {flow === "invited"
            ? "Where should sessions be stored on this machine?"
            : "Pick a Pro Tools session or watch a whole folder for auto-detection."}
        </p>

        {sessionPath ? (
          <div className="bg-[#2c2c2e] rounded-lg px-3 py-2.5 mb-4 border border-[#30d158]/30">
            <div className="flex items-center gap-2">
              <span className="text-[#30d158] text-sm">{"\u2713"}</span>
              <div>
                <div className="text-xs text-[#f5f5f7]">Added</div>
                <div className="text-[10px] text-[#98989d] font-mono truncate">
                  {sessionPath}
                </div>
              </div>
            </div>
          </div>
        ) : (
          <div className="space-y-2 mb-4">
            <button
              onClick={handleWatchFolder}
              disabled={loading}
              className="w-full flex items-center gap-3 px-3 py-3 bg-[#2c2c2e] border border-[#3a3a3c] rounded-lg hover:border-[#0a84ff]/50 transition-colors text-left"
            >
              <span className="text-lg">{"\uD83D\uDCC1"}</span>
              <div>
                <div className="text-xs font-medium text-[#f5f5f7]">
                  Watch a folder
                </div>
                <div className="text-[10px] text-[#98989d]">
                  Auto-detect Pro Tools sessions in a directory
                </div>
              </div>
            </button>

            <button
              onClick={handleAddSession}
              disabled={loading}
              className="w-full flex items-center gap-3 px-3 py-3 bg-[#2c2c2e] border border-[#3a3a3c] rounded-lg hover:border-[#0a84ff]/50 transition-colors text-left"
            >
              <span className="text-lg">{"\uD83C\uDFB9"}</span>
              <div>
                <div className="text-xs font-medium text-[#f5f5f7]">
                  Add a session
                </div>
                <div className="text-[10px] text-[#98989d]">
                  Pick a specific session folder
                </div>
              </div>
            </button>
          </div>
        )}

        {error && (
          <div className="mb-3 text-xs text-[#ff453a] bg-[#3a1a18] rounded px-2 py-1.5">
            {error}
          </div>
        )}

        <div className="flex justify-between">
          <button
            onClick={() => {
              setStep(3);
              clearError();
            }}
            className="px-3 py-1.5 text-xs text-[#98989d] hover:text-[#f5f5f7] transition-colors"
          >
            Back
          </button>
          <div className="flex items-center gap-2">
            {!sessionPath && (
              <button
                onClick={handleNext}
                className="px-3 py-1.5 text-xs text-[#98989d] hover:text-[#f5f5f7] transition-colors"
              >
                Skip for now
              </button>
            )}
            {sessionPath && (
              <button
                onClick={handleNext}
                className="px-4 py-1.5 text-xs font-medium bg-[#0a84ff] text-white rounded-md hover:bg-[#0a84ff]/80 transition-colors"
              >
                {flow === "admin" ? "Next" : "Done -- Start Syncing"}
              </button>
            )}
          </div>
        </div>
      </div>
    );
  }

  // ---- Step 5: Invite Partner (Admin only) ----
  function renderStep5() {
    async function handleGenerateInvite() {
      setLoading(true);
      setError(null);
      try {
        const result = await createInvite();
        setInviteResult(result);
      } catch (err) {
        setError(
          err instanceof Error ? err.message : "Failed to create invite",
        );
      }
      setLoading(false);
    }

    async function handleCopy() {
      if (!inviteResult) return;
      try {
        await navigator.clipboard.writeText(inviteResult.invite_url);
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      } catch {
        // Clipboard API fallback
        setError("Failed to copy. Manually select the link above.");
      }
    }

    return (
      <div className="px-6 py-6">
        <h2 className="text-sm font-semibold text-[#f5f5f7] mb-1">
          Invite Your Partner
        </h2>
        <p className="text-xs text-[#98989d] mb-5">
          Share this link with your partner to connect their machine. They will
          not need R2 credentials -- SessionSync handles that.
        </p>

        {!inviteResult ? (
          <button
            onClick={handleGenerateInvite}
            disabled={loading}
            className="w-full py-2.5 text-xs font-medium bg-[#0a84ff] text-white rounded-md hover:bg-[#0a84ff]/80 transition-colors disabled:opacity-50 flex items-center justify-center gap-1.5"
          >
            {loading && (
              <span className="w-3 h-3 border border-white/30 border-t-white rounded-full animate-spin" />
            )}
            Generate Invite Link
          </button>
        ) : (
          <div className="space-y-3">
            <div className="flex items-center gap-2 bg-[#2c2c2e] border border-[#3a3a3c] rounded-lg px-3 py-2">
              <input
                type="text"
                readOnly
                value={inviteResult.invite_url}
                className="flex-1 bg-transparent text-[10px] text-[#f5f5f7] font-mono focus:outline-none select-all"
              />
              <button
                onClick={handleCopy}
                className={`shrink-0 px-2 py-1 text-[10px] font-medium rounded transition-colors ${
                  copied
                    ? "bg-[#30d158]/20 text-[#30d158]"
                    : "bg-[#3a3a3c] text-[#f5f5f7] hover:bg-[#48484a]"
                }`}
              >
                {copied ? "Copied!" : "Copy"}
              </button>
            </div>
            <div className="text-[10px] text-[#636366]">
              This link expires in 7 days and can only be used once.
            </div>
          </div>
        )}

        {error && (
          <div className="mt-3 text-xs text-[#ff453a] bg-[#3a1a18] rounded px-2 py-1.5">
            {error}
          </div>
        )}

        <div className="flex justify-between mt-6">
          <button
            onClick={() => {
              setStep(4);
              clearError();
            }}
            className="px-3 py-1.5 text-xs text-[#98989d] hover:text-[#f5f5f7] transition-colors"
          >
            Back
          </button>
          <button
            onClick={handleFinish}
            className="px-4 py-1.5 text-xs font-medium bg-[#0a84ff] text-white rounded-md hover:bg-[#0a84ff]/80 transition-colors"
          >
            Done
          </button>
        </div>
      </div>
    );
  }

  // ---- Progress indicator ----
  const totalSteps = flow === "admin" ? 5 : 4;

  return (
    <div className="flex flex-col h-screen bg-[#1c1c1e]">
      {/* Progress dots */}
      {step > 1 && (
        <div className="flex items-center justify-center gap-1.5 py-3 border-b border-[#2c2c2e]">
          {Array.from({ length: totalSteps }, (_, i) => (
            <div
              key={i}
              className={`w-1.5 h-1.5 rounded-full transition-colors ${
                i + 1 <= step ? "bg-[#0a84ff]" : "bg-[#3a3a3c]"
              }`}
            />
          ))}
        </div>
      )}

      {/* Step content */}
      <div className="flex-1 overflow-y-auto">
        {step === 1 && renderStep1()}
        {step === 2 && renderStep2()}
        {step === 3 && flow === "admin" && renderStep3Admin()}
        {step === 3 && flow === "invited" && renderStep3Invited()}
        {step === 4 && renderStep4()}
        {step === 5 && renderStep5()}
      </div>
    </div>
  );
}
