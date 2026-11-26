import { useEffect, useState } from "react";
import { ChannelRuntimeSnapshot } from "../types";

interface ManualOverrideFormProps {
  channels: ChannelRuntimeSnapshot[];
  onApplied: () => Promise<void>;
}

export function ManualOverrideForm({ channels, onApplied }: ManualOverrideFormProps) {
  const [ovChannel, setOvChannel] = useState("");
  const [ovValue, setOvValue] = useState("");
  const [ovNote, setOvNote] = useState("Studio producer read");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (channels.length > 0 && !channels.some((c) => c.config.id === ovChannel)) {
      setOvChannel(channels[0].config.id);
    }
  }, [ovChannel, channels]);

  const submitOverride = async (e: React.FormEvent) => {
    e.preventDefault();
    const num = Number(ovValue);
    if (!ovChannel || !Number.isFinite(num) || num < 0) {
      setError("Enter a valid non-negative viewer count");
      return;
    }
    setError(null);
    setSaving(true);
    try {
      const res = await fetch("/api/overrides", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          channel_id: ovChannel,
          metric_name: "ccv",
          bucket_utc: null,
          value: num,
          operator_note: ovNote,
        }),
      });
      if (!res.ok) {
        const body = await res.json().catch(() => ({ error: "Override rejected" }));
        setError(body.error ?? "Override rejected");
        return;
      }
      setOvValue("");
      await onApplied();
    } finally {
      setSaving(false);
    }
  };

  return (
    <form
      onSubmit={submitOverride}
      className="space-y-3 rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] p-5"
    >
      <h2 className="text-sm font-medium text-[var(--text-primary)]">Manual Studio Override</h2>
      <p className="text-xs text-[var(--text-secondary)]">
        Replace unavailable viewer counts with official studio dashboard reads
      </p>

      {error && (
        <div role="alert" className="text-xs text-[#e5484d]">
          {error}
        </div>
      )}

      <div>
        <label htmlFor="ov-channel" className="mb-1 block text-xs text-[var(--text-muted)]">
          Channel
        </label>
        <select
          id="ov-channel"
          value={ovChannel}
          onChange={(e) => setOvChannel(e.target.value)}
          className="h-9 w-full rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-sm text-[var(--text-primary)]"
        >
          {channels.map((ch) => (
            <option key={ch.config.id} value={ch.config.id}>
              {ch.config.name}
            </option>
          ))}
        </select>
      </div>

      <div>
        <label htmlFor="ov-val" className="mb-1 block text-xs text-[var(--text-muted)]">
          Official Live Viewers
        </label>
        <input
          id="ov-val"
          type="number"
          min={0}
          required
          value={ovValue}
          onChange={(e) => setOvValue(e.target.value)}
          placeholder="12500"
          className="h-9 w-full rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-sm font-mono-tabular text-[var(--text-primary)]"
        />
      </div>

      <div>
        <label htmlFor="ov-note" className="mb-1 block text-xs text-[var(--text-muted)]">
          Audit Note
        </label>
        <input
          id="ov-note"
          type="text"
          value={ovNote}
          onChange={(e) => setOvNote(e.target.value)}
          className="h-9 w-full rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-sm text-[var(--text-primary)]"
        />
      </div>

      <button
        type="submit"
        disabled={saving}
        className="h-9 w-full rounded-lg bg-[var(--text-primary)] text-sm font-medium text-[var(--bg-rail)] transition-opacity hover:opacity-90 disabled:opacity-50"
      >
        {saving ? "Applying Override..." : "Apply Studio Override"}
      </button>
    </form>
  );
}
