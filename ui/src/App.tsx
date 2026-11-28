import { useCallback, useEffect, useRef, useState } from "react";
import { AccountingView } from "./components/AccountingView";
import { BrandLogo } from "./components/BrandLogo";
import { ClipsIncidentsView } from "./components/ClipsIncidentsView";
import { LiveWallView } from "./components/LiveWallView";
import { RuntimeSettingsView } from "./components/RuntimeSettingsView";
import { ShowSetupView } from "./components/ShowSetupView";
import { NavView, SidebarNav } from "./components/SidebarNav";
import { TopBar } from "./components/TopBar";
import { usePflAudio } from "./hooks/usePflAudio";
import { usePgmWatchSocket } from "./hooks/usePgmWatchSocket";
import { applyTheme, readStoredTheme, ThemeMode } from "./utils/theme";

export function App() {
  const [theme, setTheme] = useState<ThemeMode>(() => readStoredTheme());
  const [serverStopped, setServerStopped] = useState(false);

  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  if (serverStopped) {
    return (
      <div className="flex min-h-screen items-center justify-center bg-[var(--bg-page)] p-6 text-[var(--text-primary)]">
        <div className="w-full max-w-md rounded-xl border border-[var(--border-strong)] bg-[var(--bg-card)] p-6 text-center">
          <BrandLogo size={42} variant="badge" className="mx-auto mb-3" />
          <h1 className="text-lg font-medium">PGMWatch Server Stopped</h1>
          <p className="mt-2 text-sm text-[var(--text-secondary)]">
            All broadcast monitors and worker processes have exited cleanly. You can close this browser tab.
          </p>
        </div>
      </div>
    );
  }

  return (
    <MonitorConsole
      theme={theme}
      onChangeTheme={setTheme}
      onServerStopped={() => setServerStopped(true)}
    />
  );
}

interface MonitorConsoleProps {
  theme: ThemeMode;
  onChangeTheme: (next: ThemeMode) => void;
  onServerStopped: () => void;
}

function MonitorConsole({ theme, onChangeTheme, onServerStopped }: MonitorConsoleProps) {
  const [view, setView] = useState<NavView>("wall");
  const [mobileOpen, setMobileOpen] = useState(false);
  const { ensureContext, setAllowedChannel, stopAll, pushPcmPacket, muted, setMuted } =
    usePflAudio();
  const { snapshot, connected, error, refresh } = usePgmWatchSocket(pushPcmPacket);
  const [actionError, setActionError] = useState<string | null>(null);
  const [mseAudioChannels, setMseAudioChannels] = useState<Record<string, boolean>>({});
  const [shuttingDown, setShuttingDown] = useState(false);

  const pflChannelId = snapshot?.active_pfl_channel_id ?? null;
  const pflChannelIdRef = useRef<string | null>(null);
  pflChannelIdRef.current = pflChannelId;
  const mseAudioRef = useRef(mseAudioChannels);
  mseAudioRef.current = mseAudioChannels;

  const handleShutdownServer = useCallback(async () => {
    setShuttingDown(true);
    stopAll();
    try {
      const res = await fetch("/api/server/shutdown", { method: "POST" });
      if (!res.ok) {
        setActionError(`Failed to stop server (HTTP ${res.status})`);
        setShuttingDown(false);
        return;
      }
      onServerStopped();
    } catch {
      onServerStopped();
    }
  }, [onServerStopped, stopAll]);

  const handleMseAudioChange = useCallback((id: string, hasMseAudio: boolean) => {
    setMseAudioChannels((prev) => {
      if (Boolean(prev[id]) === hasMseAudio) return prev;
      return { ...prev, [id]: hasMseAudio };
    });
  }, []);

  const pflHandledByMse = Boolean(pflChannelId && mseAudioChannels[pflChannelId]);

  useEffect(() => {
    // When a channel's live MSE video carries its own muxed AAC audio track, let <video> own PFL
    // output for sample-accurate 0ms A/V lip-sync and mute WebSocket PCM to prevent double-audio/echo.
    setAllowedChannel(pflHandledByMse ? null : pflChannelId);
  }, [pflChannelId, pflHandledByMse, setAllowedChannel]);

  const postAction = useCallback(
    async (url: string, body?: unknown) => {
      try {
        const res = await fetch(url, {
          method: "POST",
          headers: body ? { "Content-Type": "application/json" } : undefined,
          body: body ? JSON.stringify(body) : undefined,
        });
        if (!res.ok) {
          const err = await res.json().catch(() => ({ error: `HTTP ${res.status}` }));
          setActionError(err.error ?? `Action failed (${res.status})`);
          return;
        }
        setActionError(null);
        await refresh();
      } catch (err) {
        setActionError(err instanceof Error ? err.message : "Network request failed");
      }
    },
    [refresh]
  );

  const handleAck = useCallback(
    (id: string) => void postAction(`/api/channels/${encodeURIComponent(id)}/ack`),
    [postAction]
  );
  const handleAckAll = useCallback(() => postAction("/api/ack_all"), [postAction]);
  const handleRetryOffline = useCallback(
    () => postAction("/api/channels/retry_offline"),
    [postAction]
  );
  const handleToggleChannelEnabled = useCallback(
    (id: string, enabled: boolean) =>
      postAction(`/api/channels/${encodeURIComponent(id)}/enabled`, { enabled }),
    [postAction]
  );
  const handleAddBookmark = useCallback(
    (channelId: string | null, note: string) =>
      postAction("/api/bookmarks", { channel_id: channelId, note }),
    [postAction]
  );
  const handleForceClip = useCallback(
    (id: string) => void postAction(`/api/channels/${encodeURIComponent(id)}/clip`),
    [postAction]
  );
  const handleTogglePfl = useCallback(
    (id: string) => {
      const turningOff = pflChannelIdRef.current === id;
      if (turningOff) {
        setAllowedChannel(null);
        stopAll();
      } else {
        ensureContext();
        setAllowedChannel(mseAudioRef.current[id] ? null : id);
      }
      void postAction(`/api/channels/${encodeURIComponent(id)}/pfl`);
    },
    [ensureContext, postAction, setAllowedChannel, stopAll]
  );
  const handleResetLufs = useCallback(
    (id: string) => postAction(`/api/channels/${encodeURIComponent(id)}/lufs/reset`),
    [postAction]
  );
  const handleSetLufsTarget = useCallback(
    (id: string, target_lufs: number | null, lufs_tolerance: number | null) =>
      postAction(`/api/channels/${encodeURIComponent(id)}/lufs/target`, {
        target_lufs,
        lufs_tolerance,
      }),
    [postAction]
  );
  const handleResetAllLufs = useCallback(() => postAction("/api/lufs/reset"), [postAction]);
  const handleToggleGroupIntermission = useCallback(
    (groupId: string, active: boolean) =>
      void postAction("/api/intermission", { group_id: groupId, active }),
    [postAction]
  );
  const handleLoginFallback = useCallback(
    (id: string) => void postAction(`/api/channels/${encodeURIComponent(id)}/auth/login`),
    [postAction]
  );
  const handleWipeAuth = useCallback(
    (id: string) => void postAction(`/api/channels/${encodeURIComponent(id)}/auth/wipe`),
    [postAction]
  );
  const handleRetryChannel = useCallback(
    (id: string) => void postAction(`/api/channels/${encodeURIComponent(id)}/retry`),
    [postAction]
  );
  const handleNavigateSetup = useCallback(() => setView("setup"), []);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey || e.shiftKey) return;
      const target = e.target as HTMLElement | null;
      if (
        target &&
        (["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName) || target.isContentEditable)
      ) {
        return;
      }
      if (e.key === "1") setView("wall");
      if (e.key === "2") setView("accounting");
      if (e.key === "3") setView("clips");
      if (e.key === "4") setView("setup");
      if (e.key === "5") setView("settings");
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const openIncidents = (snapshot?.incidents ?? []).filter(
    (i) => !i.acknowledged || i.resolved_at === null
  ).length;

  return (
    <div className="flex h-screen w-screen overflow-hidden bg-[var(--bg-page)] text-[var(--text-primary)]">
      <a href="#main-content" className="skip-link">
        Skip to main content
      </a>

      <SidebarNav
        activeView={view}
        onSelectView={setView}
        connected={connected}
        activePflChannel={snapshot?.active_pfl_channel_id ?? null}
        pflMuted={muted}
        onTogglePflMute={() => {
          ensureContext();
          setMuted((m) => !m);
        }}
        incidentOpenCount={openIncidents}
        mobileOpen={mobileOpen}
        onCloseMobile={() => setMobileOpen(false)}
        theme={theme}
        onChangeTheme={onChangeTheme}
        onShutdownServer={handleShutdownServer}
        shuttingDown={shuttingDown}
      />

      <div className="flex min-w-0 flex-1 flex-col">
        <TopBar
          snapshot={snapshot}
          onToggleGlobalIntermission={(active) =>
            void postAction("/api/intermission", { group_id: null, active })
          }
          onStartMatch={(label) => void postAction("/api/matches/start", { label })}
          onEndMatch={() => void postAction("/api/matches/end")}
          onOpenMobileMenu={() => setMobileOpen(true)}
          onRenameShow={(title) => postAction("/api/show/title", { title })}
        />

        <main id="main-content" className="flex-1 overflow-y-auto p-2 sm:p-3">
          <div className="w-full">
            {error && (
              <div
                role="alert"
                className="mb-4 rounded-lg border border-[var(--negative)]/40 bg-[var(--negative-soft)] px-4 py-2 text-xs text-[var(--negative-text)]"
              >
                Supervisor unreachable: {error}
              </div>
            )}

            {actionError && (
              <div
                role="alert"
                className="mb-4 flex items-center justify-between gap-3 rounded-lg border border-[var(--warning)]/40 bg-[var(--warning-soft)] px-4 py-2 text-xs text-[var(--warning-text)]"
              >
                <span>Action error: {actionError}</span>
                <button
                  type="button"
                  onClick={() => setActionError(null)}
                  className="rounded border border-[var(--warning)]/35 px-2 py-0.5 text-[11px] hover:bg-[var(--warning)]/15"
                >
                  Dismiss
                </button>
              </div>
            )}

            {view === "wall" && (
              <LiveWallView
                snapshot={snapshot}
                globalPflMuted={muted}
                onMseAudioChange={handleMseAudioChange}
                onAck={handleAck}
                onAckAll={handleAckAll}
                onRetryOffline={handleRetryOffline}
                onToggleChannelEnabled={handleToggleChannelEnabled}
                onAddBookmark={handleAddBookmark}
                onForceClip={handleForceClip}
                onTogglePfl={handleTogglePfl}
                onResetLufs={handleResetLufs}
                onSetLufsTarget={handleSetLufsTarget}
                onResetAllLufs={handleResetAllLufs}
                onToggleGroupIntermission={handleToggleGroupIntermission}
                onLoginFallback={handleLoginFallback}
                onWipeAuth={handleWipeAuth}
                onRetryChannel={handleRetryChannel}
                onNavigateSetup={handleNavigateSetup}
              />
            )}

            {view === "accounting" && (
              <AccountingView snapshot={snapshot} onRefresh={() => void refresh()} />
            )}

            {view === "clips" && (
              <ClipsIncidentsView
                snapshot={snapshot}
                onAck={(id) => void postAction(`/api/channels/${encodeURIComponent(id)}/ack`)}
                onRefresh={() => void refresh()}
              />
            )}

            {view === "setup" && (
              <ShowSetupView snapshot={snapshot} onRefresh={() => void refresh()} />
            )}

            {view === "settings" && <RuntimeSettingsView />}
          </div>
        </main>
      </div>
    </div>
  );
}

export default App;
