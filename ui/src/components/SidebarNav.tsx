import { useState } from "react";
import { BarChart3, Cpu, Film, Monitor, Moon, Power, Sliders, Sun, Volume2, VolumeX, X } from "lucide-react";
import type { ThemeMode } from "../utils/theme";
import { BrandLogo } from "./BrandLogo";

export type NavView = "wall" | "accounting" | "clips" | "setup" | "settings";

interface SidebarNavProps {
  activeView: NavView;
  onSelectView: (view: NavView) => void;
  connected: boolean;
  activePflChannel: string | null;
  pflMuted: boolean;
  onTogglePflMute: () => void;
  incidentOpenCount: number;
  mobileOpen: boolean;
  onCloseMobile: () => void;
  theme: ThemeMode;
  onChangeTheme: (theme: ThemeMode) => void;
  onShutdownServer: () => Promise<void> | void;
  shuttingDown?: boolean;
}

const OPERATIONS_NAV: { id: NavView; label: string; shortcut: string; Icon: typeof Monitor }[] = [
  { id: "wall", label: "Live Wall", shortcut: "1", Icon: Monitor },
  { id: "accounting", label: "Accounting", shortcut: "2", Icon: BarChart3 },
  { id: "clips", label: "Clips & Incidents", shortcut: "3", Icon: Film },
  { id: "setup", label: "Show Setup", shortcut: "4", Icon: Sliders },
];

const SYSTEM_NAV: { id: NavView; label: string; shortcut: string; Icon: typeof Monitor }[] = [
  { id: "settings", label: "Hardware & Runtime", shortcut: "5", Icon: Cpu },
];

export function SidebarNav({
  activeView,
  onSelectView,
  connected,
  activePflChannel,
  pflMuted,
  onTogglePflMute,
  incidentOpenCount,
  mobileOpen,
  onCloseMobile,
  theme,
  onChangeTheme,
  onShutdownServer,
  shuttingDown = false,
}: SidebarNavProps) {
  const [confirmShutdown, setConfirmShutdown] = useState(false);
  const renderNavItem = ({ id, label, shortcut, Icon }: (typeof OPERATIONS_NAV)[number]) => {
    const active = activeView === id;
    return (
      <button
        key={id}
        type="button"
        onClick={() => {
          onSelectView(id);
          onCloseMobile();
        }}
        aria-current={active ? "page" : undefined}
        className={`flex h-9 w-full items-center justify-between rounded-lg border px-2.5 text-sm transition-colors ${
          active
            ? "border-[var(--border-hairline)] bg-[var(--bg-raised)] font-medium text-[var(--text-primary)]"
            : "border-transparent text-[var(--text-secondary)] hover:bg-[var(--bg-card)] hover:text-[var(--text-primary)]"
        }`}
      >
        <span className="flex items-center gap-2.5 truncate">
          <span className="inline-flex w-4 shrink-0 justify-center">
            <Icon
              size={16}
              strokeWidth={1.5}
              className={active ? "text-[var(--text-primary)]" : "text-[var(--text-muted)]"}
            />
          </span>
          <span className="truncate">{label}</span>
        </span>
        {id === "clips" && incidentOpenCount > 0 ? (
          <span className="font-mono-tabular text-xs text-[var(--negative-text)]">
            {incidentOpenCount}
          </span>
        ) : (
          <kbd className="rounded border border-[var(--border-hairline)] bg-[var(--bg-rail)] px-1.5 py-0.5 text-[11px] font-mono-tabular text-[var(--text-muted)]">
            {shortcut}
          </kbd>
        )}
      </button>
    );
  };

  const navContent = (
    <div className="flex h-full flex-col justify-between p-3">
      <div>
        <div className="flex items-center justify-between pb-4">
          <div className="flex items-center gap-2.5">
            <BrandLogo size={30} variant="badge" />
            <div className="leading-tight">
              <div className="text-[15px] font-semibold tracking-tight text-[var(--text-primary)]">
                PGMWatch
              </div>
              <div className="text-[11px] text-[var(--text-muted)]">Broadcast Monitor</div>
            </div>
          </div>
          <button
            type="button"
            onClick={onCloseMobile}
            aria-label="Close navigation menu"
            className="inline-flex h-8 w-8 items-center justify-center rounded-lg text-[var(--text-secondary)] hover:bg-[var(--bg-raised)] lg:hidden"
          >
            <X size={16} strokeWidth={1.5} />
          </button>
        </div>

        <nav aria-label="Main navigation">
          <div className="mb-2 text-xs text-[var(--text-muted)]">Operations</div>
          <div className="flex flex-col gap-1">{OPERATIONS_NAV.map(renderNavItem)}</div>

          <div className="mb-2 mt-4 text-xs text-[var(--text-muted)]">System</div>
          <div className="flex flex-col gap-1">{SYSTEM_NAV.map(renderNavItem)}</div>
        </nav>
      </div>

      <div className="flex flex-col gap-3 border-t border-[var(--border-hairline)] pt-4 text-xs">
        <div
          role="group"
          aria-label="Color appearance"
          className="grid grid-cols-2 gap-1 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-page)] p-1"
        >
          <button
            type="button"
            onClick={() => onChangeTheme("dark")}
            aria-pressed={theme === "dark"}
            className={`inline-flex h-7 items-center justify-center gap-1.5 rounded-md text-xs font-medium transition-colors ${
              theme === "dark"
                ? "bg-[var(--bg-card)] text-[var(--text-primary)] shadow-sm"
                : "text-[var(--text-muted)] hover:text-[var(--text-primary)]"
            }`}
          >
            <Moon size={13} strokeWidth={1.5} aria-hidden="true" />
            <span>Dark</span>
          </button>
          <button
            type="button"
            onClick={() => onChangeTheme("light")}
            aria-pressed={theme === "light"}
            className={`inline-flex h-7 items-center justify-center gap-1.5 rounded-md text-xs font-medium transition-colors ${
              theme === "light"
                ? "bg-[var(--bg-card)] text-[var(--text-primary)] shadow-sm"
                : "text-[var(--text-muted)] hover:text-[var(--text-primary)]"
            }`}
          >
            <Sun size={13} strokeWidth={1.5} aria-hidden="true" />
            <span>Light</span>
          </button>
        </div>

        <div className="flex items-center justify-between rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-card)] px-3 py-2">
          <div className="min-w-0">
            <div className="text-[var(--text-muted)]">Audio Monitor (PFL)</div>
            <div className="truncate font-mono-tabular text-[var(--text-primary)]">
              {activePflChannel ?? "Idle"}
            </div>
          </div>
          <button
            type="button"
            onClick={onTogglePflMute}
            aria-label={pflMuted ? "Unmute PFL output" : "Mute PFL output"}
            className="inline-flex h-7 w-7 items-center justify-center rounded bg-[var(--bg-raised)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
          >
            {pflMuted ? <VolumeX size={14} strokeWidth={1.5} /> : <Volume2 size={14} strokeWidth={1.5} />}
          </button>
        </div>

        <div className="flex items-center justify-between text-[var(--text-muted)]" role="status" aria-live="polite">
          <span>Supervisor link</span>
          <span className="flex items-center gap-1.5 font-mono-tabular text-[var(--text-secondary)]">
            <span
              className={`h-2 w-2 rounded-full ${connected ? "bg-[var(--positive)]" : "bg-[var(--negative)]"}`}
              aria-hidden="true"
            />
            {connected ? "Connected" : "Reconnecting"}
          </span>
        </div>

        {confirmShutdown ? (
          <div className="rounded-lg border border-[var(--negative)]/45 bg-[var(--negative-soft)] p-2.5 space-y-2">
            <div className="text-[11px] font-medium text-[var(--negative-text)]">
              Stop PGMWatch server and all feed monitors?
            </div>
            <div className="flex items-center gap-1.5">
              <button
                type="button"
                disabled={shuttingDown}
                onClick={() => {
                  setConfirmShutdown(false);
                  void onShutdownServer();
                }}
                className="inline-flex h-7 flex-1 items-center justify-center rounded bg-[#dc2626] px-2 text-[11px] font-medium text-white hover:opacity-90 disabled:opacity-50"
              >
                {shuttingDown ? "Stopping..." : "Confirm Stop"}
              </button>
              <button
                type="button"
                onClick={() => setConfirmShutdown(false)}
                className="inline-flex h-7 items-center justify-center rounded border border-[var(--border-hairline)] bg-[var(--bg-card)] px-2.5 text-[11px] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
              >
                Cancel
              </button>
            </div>
          </div>
        ) : (
          <button
            type="button"
            disabled={!connected || shuttingDown}
            onClick={() => setConfirmShutdown(true)}
            className="inline-flex h-8 w-full items-center justify-center gap-1.5 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-card)] px-3 text-xs font-medium text-[var(--text-secondary)] transition-colors hover:border-[var(--negative)]/50 hover:bg-[var(--negative-soft)] hover:text-[var(--negative-text)] disabled:opacity-45 disabled:cursor-not-allowed"
          >
            <Power size={13} strokeWidth={1.5} aria-hidden="true" />
            <span>Stop Server</span>
          </button>
        )}

        <div className="border-t border-[var(--border-hairline)] pt-2 text-center space-y-1">
          <a
            href="https://github.com/Ballas43"
            target="_blank"
            rel="noopener noreferrer"
            className="credit-link"
            title="github.com/Ballas43"
          >
            <span>Project by</span>
            <img src="/personal_logo.webp" alt="Ballas43" className="credit-logo" />
          </a>
          <div className="text-[10px] text-[var(--text-muted)]">
            <a
              href="https://github.com/Ballas43/PGMWatch"
              target="_blank"
              rel="noopener noreferrer"
              className="hover:text-[var(--text-secondary)] underline underline-offset-2"
              title="AGPL-3.0 Corresponding Source"
            >
              AGPL-3.0 · Source
            </a>
          </div>
        </div>
      </div>
    </div>
  );

  return (
    <>
      <aside className="hidden w-52 shrink-0 border-r border-[var(--border-hairline)] bg-[var(--bg-rail)] lg:block">
        {navContent}
      </aside>
      {mobileOpen && (
        <div className="fixed inset-0 z-50 flex lg:hidden">
          <div className="w-52 border-r border-[var(--border-hairline)] bg-[var(--bg-rail)]">
            {navContent}
          </div>
          <button
            type="button"
            aria-label="Dismiss navigation drawer"
            onClick={onCloseMobile}
            className="flex-1 bg-black/60"
          />
        </div>
      )}
    </>
  );
}
