import { useCallback, useEffect, useState } from "react";
import { CheckCircle2, ExternalLink, KeyRound, Loader2, RefreshCw, Unlink } from "lucide-react";

interface PlatformAuthItem {
  platform: "facebook" | "tiktok";
  label: string;
  has_channels: boolean;
  profile_ready: boolean;
  connected: boolean;
  unlocks: string;
}

interface PlatformAuthPanelProps {
  onSynced?: () => void;
}

export function PlatformAuthPanel({ onSynced }: PlatformAuthPanelProps) {
  const [platforms, setPlatforms] = useState<PlatformAuthItem[]>([]);
  const [awaitingSync, setAwaitingSync] = useState<Record<string, boolean>>({});
  const [busyPlatform, setBusyPlatform] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<Record<string, { kind: "ok" | "err" | "info"; text: string }>>({});

  const loadStatus = useCallback(async () => {
    try {
      const res = await fetch("/api/auth/platforms");
      if (res.ok) {
        const data = await res.json();
        setPlatforms(Array.isArray(data.platforms) ? data.platforms : []);
      }
    } catch {
      // ignore transient network errors
    }
  }, []);

  useEffect(() => {
    void loadStatus();
  }, [loadStatus]);

  const visible = platforms.filter((p) => p.has_channels);
  if (visible.length === 0) return null;

  const handleSignIn = async (item: PlatformAuthItem) => {
    setBusyPlatform(item.platform);
    setFeedback((prev) => ({ ...prev, [item.platform]: { kind: "info", text: `Opening ${item.label} login in isolated Edge...` } }));
    try {
      const res = await fetch(`/api/auth/platforms/${item.platform}/login`, { method: "POST" });
      if (!res.ok) {
        const msg = await res.text();
        setFeedback((prev) => ({
          ...prev,
          [item.platform]: { kind: "err", text: msg || `Failed to launch ${item.label} login window.` },
        }));
        return;
      }
      setAwaitingSync((prev) => ({ ...prev, [item.platform]: true }));
      setFeedback((prev) => ({
        ...prev,
        [item.platform]: {
          kind: "info",
          text: `Finish signing in to ${item.label} in the Edge window, then click Sync Session (the login window will close automatically).`,
        },
      }));
    } catch (err) {
      setFeedback((prev) => ({
        ...prev,
        [item.platform]: {
          kind: "err",
          text: err instanceof Error ? err.message : "Network error",
        },
      }));
    } finally {
      setBusyPlatform(null);
    }
  };

  const handleSync = async (item: PlatformAuthItem) => {
    setBusyPlatform(item.platform);
    try {
      const res = await fetch(`/api/auth/platforms/${item.platform}/sync`, { method: "POST" });
      if (!res.ok) {
        const msg = await res.text();
        setFeedback((prev) => ({
          ...prev,
          [item.platform]: { kind: "err", text: msg || "Could not sync session yet." },
        }));
        return;
      }
      setAwaitingSync((prev) => ({ ...prev, [item.platform]: false }));
      setFeedback((prev) => ({
        ...prev,
        [item.platform]: { kind: "ok", text: `${item.label} session synced. Refreshing engagement metrics...` },
      }));
      await loadStatus();
      onSynced?.();
    } catch (err) {
      setFeedback((prev) => ({
        ...prev,
        [item.platform]: {
          kind: "err",
          text: err instanceof Error ? err.message : "Network error",
        },
      }));
    } finally {
      setBusyPlatform(null);
    }
  };

  const handleDisconnect = async (item: PlatformAuthItem) => {
    setBusyPlatform(item.platform);
    try {
      const res = await fetch(`/api/auth/platforms/${item.platform}/wipe`, { method: "POST" });
      if (res.ok) {
        setAwaitingSync((prev) => ({ ...prev, [item.platform]: false }));
        setFeedback((prev) => ({
          ...prev,
          [item.platform]: { kind: "info", text: `${item.label} session cleared.` },
        }));
        await loadStatus();
        onSynced?.();
      }
    } finally {
      setBusyPlatform(null);
    }
  };

  return (
    <section
      aria-labelledby="platform-sessions-heading"
      className="rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] px-4 py-3"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <KeyRound size={14} strokeWidth={1.5} className="text-[var(--text-muted)]" aria-hidden="true" />
          <h2 id="platform-sessions-heading" className="text-xs font-semibold text-[var(--text-primary)]">
            Platform Sessions (Optional)
          </h2>
        </div>
        <span className="text-[11px] text-[var(--text-muted)]">
          Sign in via isolated Edge to unlock login-gated engagement metrics
        </span>
      </div>

      <ul role="list" className="mt-2.5 divide-y divide-[var(--border-hairline)]">
        {visible.map((item) => {
          const isBusy = busyPlatform === item.platform;
          const needsSync = Boolean(awaitingSync[item.platform]);
          const note = feedback[item.platform];
          return (
            <li key={item.platform} className="py-2 first:pt-0 last:pb-0">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="text-xs font-medium text-[var(--text-primary)]">{item.label}</span>
                    {item.connected ? (
                      <span className="inline-flex items-center gap-1 rounded border border-[var(--positive)]/40 bg-[var(--positive-soft)] px-1.5 py-0.5 text-[10px] font-medium text-[var(--positive-text)]">
                        <CheckCircle2 size={11} strokeWidth={1.75} aria-hidden="true" />
                        <span>Connected</span>
                      </span>
                    ) : needsSync ? (
                      <span className="inline-flex items-center gap-1 rounded border border-[var(--warning)]/40 bg-[var(--warning-soft)] px-1.5 py-0.5 text-[10px] font-medium text-[var(--warning-text)]">
                        <span>Awaiting Sync</span>
                      </span>
                    ) : (
                      <span className="inline-flex items-center rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-1.5 py-0.5 text-[10px] font-medium text-[var(--text-muted)]">
                        Not Signed In
                      </span>
                    )}
                  </div>
                  <p className="mt-0.5 text-[11px] text-[var(--text-muted)]">{item.unlocks}</p>
                </div>

                <div className="flex flex-wrap items-center gap-1.5">
                  {!item.connected && !needsSync && (
                    <button
                      type="button"
                      disabled={isBusy}
                      onClick={() => void handleSignIn(item)}
                      className="inline-flex h-7 items-center gap-1.5 rounded-lg border border-[var(--border-strong)] bg-[var(--bg-raised)] px-2.5 text-xs font-medium text-[var(--text-primary)] transition-colors hover:border-[var(--text-secondary)] disabled:opacity-50"
                    >
                      {isBusy ? (
                        <Loader2 size={12} className="animate-spin" aria-hidden="true" />
                      ) : (
                        <ExternalLink size={12} strokeWidth={1.5} aria-hidden="true" />
                      )}
                      <span>Sign In to {item.label}</span>
                    </button>
                  )}

                  {(needsSync || (item.profile_ready && !item.connected)) && (
                    <button
                      type="button"
                      disabled={isBusy}
                      onClick={() => void handleSync(item)}
                      className="inline-flex h-7 items-center gap-1.5 rounded-lg border border-[var(--positive)]/50 bg-[var(--positive-soft)] px-2.5 text-xs font-medium text-[var(--positive-text)] transition-colors hover:brightness-110 disabled:opacity-50"
                    >
                      {isBusy ? (
                        <Loader2 size={12} className="animate-spin" aria-hidden="true" />
                      ) : (
                        <RefreshCw size={12} strokeWidth={1.5} aria-hidden="true" />
                      )}
                      <span>Sync Session</span>
                    </button>
                  )}

                  {item.connected && (
                    <>
                      <button
                        type="button"
                        disabled={isBusy}
                        onClick={() => void handleSync(item)}
                        aria-label={`Refresh ${item.label} session cookies`}
                        className="inline-flex h-7 items-center gap-1 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 text-xs text-[var(--text-secondary)] transition-colors hover:text-[var(--text-primary)] disabled:opacity-50"
                      >
                        <RefreshCw size={12} strokeWidth={1.5} aria-hidden="true" />
                        <span>Re-sync</span>
                      </button>
                      <button
                        type="button"
                        disabled={isBusy}
                        onClick={() => void handleDisconnect(item)}
                        aria-label={`Disconnect ${item.label} session`}
                        className="inline-flex h-7 items-center gap-1 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 text-xs text-[var(--text-muted)] transition-colors hover:border-[var(--negative)]/50 hover:text-[var(--negative-text)] disabled:opacity-50"
                      >
                        <Unlink size={12} strokeWidth={1.5} aria-hidden="true" />
                        <span>Disconnect</span>
                      </button>
                    </>
                  )}
                </div>
              </div>

              {note && (
                <p
                  role="status"
                  aria-live="polite"
                  className={`mt-1.5 text-[11px] ${
                    note.kind === "err"
                      ? "text-[var(--negative-text)]"
                      : note.kind === "ok"
                      ? "text-[var(--positive-text)]"
                      : "text-[var(--text-secondary)]"
                  }`}
                >
                  {note.text}
                </p>
              )}
            </li>
          );
        })}
      </ul>
    </section>
  );
}
