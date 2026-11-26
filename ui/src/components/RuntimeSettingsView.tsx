import { useState } from "react";
import { RuntimeSettingsPanel } from "./RuntimeSettingsPanel";

export function RuntimeSettingsView() {
  const [statusMsg, setStatusMsg] = useState<{ text: string; isError?: boolean } | null>(null);

  return (
    <div className="space-y-6 animate-enter">
      <div>
        <h1 className="text-2xl font-medium tracking-tight text-[var(--text-primary)]">
          Hardware & Runtime Settings
        </h1>
        <p className="mt-1 text-sm text-[var(--text-secondary)]">
          System-wide GPU decode pinning, extraction options, resource limits, and alert tuning stored in pgmwatch.toml
        </p>
      </div>

      {statusMsg && (
        <div
          role={statusMsg.isError ? "alert" : "status"}
          aria-live="polite"
          className={`flex items-center justify-between gap-3 rounded-lg border px-3 py-2 text-xs ${
            statusMsg.isError
              ? "border-[var(--negative)]/50 bg-[var(--negative-soft)] text-[var(--negative-text)]"
              : "border-[var(--border-strong)] bg-[var(--bg-card)] text-[var(--text-primary)]"
          }`}
        >
          <span>{statusMsg.text}</span>
          <button
            type="button"
            onClick={() => setStatusMsg(null)}
            className="rounded border border-[var(--border-hairline)] px-2 py-0.5 text-[11px] hover:bg-[var(--bg-raised)]"
          >
            Dismiss
          </button>
        </div>
      )}

      <RuntimeSettingsPanel onStatus={setStatusMsg} />
    </div>
  );
}
