import { useEffect, useRef, useState } from "react";
import { SlidersHorizontal } from "lucide-react";
import { ANALYTICS_GLOSSARY } from "../utils/analyticsGlossary";
import { InfoTip } from "./InfoTip";

interface LufsTargetEditorProps {
  channelId: string;
  channelName: string;
  effectiveTarget: number;
  effectiveTolerance: number;
  isCustomOverride: boolean;
  onSetLufsTarget: (
    id: string,
    targetLufs: number | null,
    lufsTolerance: number | null
  ) => Promise<void> | void;
}

const PRESET_TARGETS = [
  { label: "-14 YT/TW", target: -14.0 },
  { label: "-16 Stream", target: -16.0 },
  { label: "-23 EBU", target: -23.0 },
  { label: "-11 Mobile", target: -11.0 },
];

export function LufsTargetEditor({
  channelId,
  channelName,
  effectiveTarget,
  effectiveTolerance,
  isCustomOverride,
  onSetLufsTarget,
}: LufsTargetEditorProps) {
  const [open, setOpen] = useState(false);
  const [targetStr, setTargetStr] = useState(effectiveTarget.toFixed(1));
  const [tolStr, setTolStr] = useState(effectiveTolerance.toFixed(1));
  const [saving, setSaving] = useState(false);
  const wrapRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) {
      setTargetStr(effectiveTarget.toFixed(1));
      setTolStr(effectiveTolerance.toFixed(1));
      return;
    }
    const onPointerDown = (e: MouseEvent) => {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("mousedown", onPointerDown);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("mousedown", onPointerDown);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [effectiveTarget, effectiveTolerance, open]);

  const apply = async (nextTarget: number | null, nextTol: number | null) => {
    if (saving) return;
    setSaving(true);
    try {
      await onSetLufsTarget(channelId, nextTarget, nextTol);
      setOpen(false);
    } finally {
      setSaving(false);
    }
  };

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const parsedTarget = Number(targetStr);
    const parsedTol = Number(tolStr);
    if (!Number.isFinite(parsedTarget) || parsedTarget < -36 || parsedTarget > -5) return;
    const validTol = Number.isFinite(parsedTol) && parsedTol >= 0.5 && parsedTol <= 10 ? parsedTol : 2.5;
    void apply(parsedTarget, validTol);
  };

  return (
    <div ref={wrapRef} className="relative">
      <InfoTip entry={ANALYTICS_GLOSSARY.lufsTarget} disabled={open}>
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          aria-expanded={open}
          aria-label={`Set target LUFS for ${channelName}`}
          className={`inline-flex h-6 items-center gap-1 rounded border px-1.5 text-[10px] font-mono-tabular transition-colors ${
            isCustomOverride
              ? "border-[var(--accent)]/50 bg-[var(--accent-soft)] text-[var(--accent-text)]"
              : "border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-secondary)] hover:border-[var(--border-strong)] hover:text-[var(--text-primary)]"
          }`}
        >
          <SlidersHorizontal size={10} strokeWidth={1.6} aria-hidden="true" />
          <span>
            Target {effectiveTarget.toFixed(1)} ±{effectiveTolerance.toFixed(1)}
          </span>
        </button>
      </InfoTip>

      {open && (
        <form
          onSubmit={handleSubmit}
          role="dialog"
          aria-label={`Target loudness settings for ${channelName}`}
          className="absolute right-0 top-7 z-30 w-64 space-y-2.5 rounded-lg border border-[var(--border-strong)] bg-[var(--bg-popover)] p-3 font-sans text-xs shadow-xl"
        >
          <div className="flex items-center justify-between">
            <span className="font-medium text-[var(--text-primary)]">Custom Loudness Target</span>
            <span className="font-mono-tabular text-[10px] text-[var(--text-muted)]">
              {isCustomOverride ? "Custom" : "Auto"}
            </span>
          </div>

          <div className="grid grid-cols-2 gap-1.5">
            {PRESET_TARGETS.map((p) => (
              <button
                key={p.label}
                type="button"
                disabled={saving}
                onClick={() => {
                  setTargetStr(p.target.toFixed(1));
                  void apply(p.target, Number(tolStr) || 2.5);
                }}
                className="rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 py-1 text-left font-mono-tabular text-[10px] text-[var(--text-secondary)] hover:border-[var(--border-strong)] hover:text-[var(--text-primary)]"
              >
                {p.label}
              </button>
            ))}
          </div>

          <div className="grid grid-cols-2 gap-2">
            <div>
              <label htmlFor={`lufs-t-${channelId}`} className="mb-0.5 block text-[10px] text-[var(--text-muted)]">
                Target (LUFS)
              </label>
              <input
                id={`lufs-t-${channelId}`}
                type="number"
                step="0.5"
                min="-36"
                max="-5"
                value={targetStr}
                onChange={(e) => setTargetStr(e.target.value)}
                className="h-7 w-full rounded border border-[var(--border-hairline)] bg-[var(--bg-rail)] px-2 font-mono-tabular text-xs text-[var(--text-primary)]"
              />
            </div>
            <div>
              <label htmlFor={`lufs-tol-${channelId}`} className="mb-0.5 block text-[10px] text-[var(--text-muted)]">
                Range (± LU)
              </label>
              <input
                id={`lufs-tol-${channelId}`}
                type="number"
                step="0.5"
                min="0.5"
                max="10"
                value={tolStr}
                onChange={(e) => setTolStr(e.target.value)}
                className="h-7 w-full rounded border border-[var(--border-hairline)] bg-[var(--bg-rail)] px-2 font-mono-tabular text-xs text-[var(--text-primary)]"
              />
            </div>
          </div>

          <div className="flex items-center justify-between gap-2 pt-0.5">
            <button
              type="button"
              disabled={saving}
              onClick={() => void apply(null, null)}
              className="h-7 rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 text-[11px] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
            >
              Auto Default
            </button>
            <div className="flex items-center gap-1.5">
              <button
                type="button"
                onClick={() => setOpen(false)}
                className="h-7 rounded px-2 text-[11px] text-[var(--text-muted)] hover:text-[var(--text-primary)]"
              >
                Cancel
              </button>
              <button
                type="submit"
                disabled={saving}
                className="h-7 rounded bg-[var(--text-primary)] px-2.5 text-[11px] font-medium text-[var(--bg-rail)] disabled:opacity-50"
              >
                Apply
              </button>
            </div>
          </div>
        </form>
      )}
    </div>
  );
}
