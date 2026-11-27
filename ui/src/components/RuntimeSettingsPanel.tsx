import { useCallback, useEffect, useMemo, useState } from "react";
import { RefreshCw, Save, Sliders } from "lucide-react";
import { InfoTip } from "./InfoTip";
import { RUNTIME_SETTINGS_GLOSSARY, type MetricGlossaryEntry } from "../utils/analyticsGlossary";
import type {
  DxgiAdapterInfo,
  PgmWatchConfig,
  RuntimeConfigResponse,
  SaveConfigResponse,
} from "../types";

interface StatusMessage {
  text: string;
  isError?: boolean;
}

interface Props {
  onStatus: (msg: StatusMessage) => void;
}

function adapterLabel(a: DxgiAdapterInfo): string {
  return `#${a.adapter_index} ${a.name} (${a.luid_hex}, ${a.vram_used_mb}/${a.vram_budget_mb} MB)`;
}

function numVal(raw: string, fallback: number): number {
  if (raw.trim() === "") return 0;
  const parsed = Number(raw);
  return Number.isFinite(parsed) ? parsed : fallback;
}

export function RuntimeSettingsPanel({ onStatus }: Props) {
  const [saved, setSaved] = useState<PgmWatchConfig | null>(null);
  const [draft, setDraft] = useState<PgmWatchConfig | null>(null);
  const [customLufsText, setCustomLufsText] = useState<string>("");
  const [adapters, setAdapters] = useState<DxgiAdapterInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [localBanner, setLocalBanner] = useState<StatusMessage | null>(null);

  const loadConfig = useCallback(async () => {
    setLoading(true);
    try {
      const res = await fetch("/api/config");
      if (!res.ok) {
        const body = await res.json().catch(() => ({ error: res.statusText }));
        throw new Error(body.error || `HTTP ${res.status}`);
      }
      const data: RuntimeConfigResponse = await res.json();
      setSaved(data.config);
      setDraft(data.config);
      setCustomLufsText(
        data.config.alerts.custom_target_lufs == null
          ? ""
          : String(data.config.alerts.custom_target_lufs)
      );
      setAdapters(data.adapters ?? []);
    } catch (err) {
      const msg = { text: `Failed to load runtime config: ${String(err)}`, isError: true };
      setLocalBanner(msg);
      onStatus(msg);
    } finally {
      setLoading(false);
    }
  }, [onStatus]);

  useEffect(() => {
    void loadConfig();
  }, [loadConfig]);

  const isDirty = useMemo(() => {
    if (!saved || !draft) return false;
    return JSON.stringify(saved) !== JSON.stringify(draft);
  }, [saved, draft]);

  async function handleSave(e: React.FormEvent) {
    e.preventDefault();
    if (!draft) return;
    setSaving(true);
    setLocalBanner(null);
    try {
      const res = await fetch("/api/config", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(draft),
      });
      const body = (await res.json().catch(() => ({}))) as Partial<SaveConfigResponse> & {
        error?: string;
      };
      if (!res.ok) {
        throw new Error(body.error || `HTTP ${res.status}`);
      }
      if (body.config) {
        setSaved(body.config);
        setDraft(body.config);
        setCustomLufsText(
          body.config.alerts.custom_target_lufs == null
            ? ""
            : String(body.config.alerts.custom_target_lufs)
        );
      }
      const msg: StatusMessage = {
        text: body.restart_required
          ? "Runtime config saved to pgmwatch.toml (restart supervisor for bind/storage changes)"
          : "Runtime config saved to pgmwatch.toml and applied live",
        isError: false,
      };
      setLocalBanner(msg);
      onStatus(msg);
    } catch (err) {
      const msg: StatusMessage = {
        text: err instanceof Error ? err.message : String(err),
        isError: true,
      };
      setLocalBanner(msg);
      onStatus(msg);
    } finally {
      setSaving(false);
    }
  }

  function renderAdapterSelect(
    id: string,
    label: string,
    value: string,
    onChange: (next: string) => void,
    tip: MetricGlossaryEntry
  ) {
    const trimmed = value.trim();
    const normalized = trimmed.toLowerCase() === "auto" ? "" : trimmed;
    const hasCustom =
      normalized !== "" &&
      !adapters.some((a) => a.luid_hex.toLowerCase() === normalized.toLowerCase());

    return (
      <div>
        <label htmlFor={id} className="mb-1 block text-xs text-[var(--text-muted)]">
          <InfoTip entry={tip} underline focusable>
            <span>{label}</span>
          </InfoTip>
        </label>
        <select
          id={id}
          value={normalized}
          onChange={(e) => onChange(e.target.value)}
          className="h-9 w-full rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-xs text-[var(--text-primary)]"
        >
          <option value="">Auto (Default Adapter)</option>
          {adapters.map((a) => (
            <option key={a.luid_hex} value={a.luid_hex}>
              {adapterLabel(a)}
            </option>
          ))}
          {hasCustom && (
            <option value={normalized}>{`Custom LUID (${normalized})`}</option>
          )}
        </select>
      </div>
    );
  }

  if (loading && !draft) {
    return (
      <section className="rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] p-5 text-xs text-[var(--text-muted)]">
        Loading runtime configuration and DXGI adapters...
      </section>
    );
  }

  if (!draft) {
    return (
      <section className="rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] p-5 flex items-center justify-between gap-3 text-xs">
        <span className="text-[var(--negative-text)]">Runtime configuration unavailable.</span>
        <button
          type="button"
          onClick={() => void loadConfig()}
          className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-[var(--border-strong)] bg-[var(--bg-raised)] px-3 text-xs font-medium text-[var(--text-primary)]"
        >
          <RefreshCw className="h-3.5 w-3.5" />
          Retry
        </button>
      </section>
    );
  }

  const inputCls =
    "h-9 w-full rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-xs font-mono-tabular text-[var(--text-primary)]";
  const selectCls =
    "h-9 w-full rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-xs text-[var(--text-primary)]";
  const labelCls = "mb-1 block text-xs text-[var(--text-muted)]";

  return (
    <section
      aria-labelledby="runtime-settings-heading"
      className="rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] p-5 space-y-5"
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2
            id="runtime-settings-heading"
            className="flex items-center gap-2 text-sm font-semibold text-[var(--text-primary)]"
          >
            <Sliders className="h-4 w-4 text-[var(--text-muted)]" />
            Runtime & Hardware Config (pgmwatch.toml)
          </h2>
          <p className="text-xs text-[var(--text-muted)]">
            Advanced PC performance, GPU assignment, YouTube extraction, and alert sensitivity settings. Hover or focus any dotted label for plain-English guidance — defaults work out of the box for most shows.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2 text-[11px] font-mono-tabular text-[var(--text-muted)]">
          <span className="rounded border border-[var(--border-hairline)] bg-[var(--bg-page)] px-2 py-1">
            {`${draft.server.bind_host}:${draft.server.port}`}
          </span>
          <span className="rounded border border-[var(--border-hairline)] bg-[var(--bg-page)] px-2 py-1">
            {`${adapters.length} GPU${adapters.length === 1 ? "" : "s"}`}
          </span>
          <button
            type="button"
            onClick={() => void loadConfig()}
            disabled={loading || saving}
            aria-label="Refresh runtime config and GPU adapters"
            className="inline-flex h-7 items-center gap-1 rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-[11px] text-[var(--text-primary)] hover:bg-[var(--bg-page)] disabled:opacity-50"
          >
            <RefreshCw className={`h-3 w-3 ${loading ? "animate-spin" : ""}`} />
            Reload
          </button>
        </div>
      </div>

      <form onSubmit={handleSave} className="space-y-5">
        <div className="space-y-2.5">
          <div>
            <h4 className="text-xs font-semibold text-[var(--text-secondary)]">
              <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.sectionMedia} underline focusable>
                <span>Media Decode & GPU Pinning</span>
              </InfoTip>
            </h4>
            <p className="mt-0.5 text-[11px] text-[var(--text-muted)]/85">
              Choose which GPU decodes video feeds, how smooth wall previews run, and how much disk space replay clips may use.
            </p>
          </div>
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
            <div>
              <label htmlFor="cfg-gpu-mode" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.gpuMode} underline focusable>
                  <span>GPU Mode</span>
                </InfoTip>
              </label>
              <select
                id="cfg-gpu-mode"
                value={draft.media.gpu_mode}
                onChange={(e) =>
                  setDraft({ ...draft, media: { ...draft.media, gpu_mode: e.target.value } })
                }
                className={selectCls}
              >
                <option value="auto">auto (NPU / iGPU preferred)</option>
                <option value="d3d11va">d3d11va (DXGI hardware decode)</option>
                <option value="cli_ffmpeg">cli_ffmpeg (CLI FFmpeg fallback)</option>
                <option value="cpu_light">cpu_light (software decode only)</option>
              </select>
            </div>

            {renderAdapterSelect(
              "cfg-master-luid",
              "Master Feed GPU",
              draft.media.master_gpu_luid,
              (val) =>
                setDraft({ ...draft, media: { ...draft.media, master_gpu_luid: val } }),
              RUNTIME_SETTINGS_GLOSSARY.masterGpuLuid
            )}

            {renderAdapterSelect(
              "cfg-simulcast-luid",
              "Backup Feed GPU",
              draft.media.simulcast_gpu_luid,
              (val) =>
                setDraft({ ...draft, media: { ...draft.media, simulcast_gpu_luid: val } }),
              RUNTIME_SETTINGS_GLOSSARY.simulcastGpuLuid
            )}

            <div>
              <label htmlFor="cfg-extra-hw-frames" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.extraHwFrames} underline focusable>
                  <span>Extra HW Frames (1–64)</span>
                </InfoTip>
              </label>
              <input
                id="cfg-extra-hw-frames"
                type="number"
                step="1"
                value={draft.media.extra_hw_frames}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    media: {
                      ...draft.media,
                      extra_hw_frames: numVal(e.target.value, draft.media.extra_hw_frames),
                    },
                  })
                }
                className={inputCls}
              />
            </div>

            <div>
              <label htmlFor="cfg-thumb-fps" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.thumbnailFps} underline focusable>
                  <span>Thumbnail FPS (0.2–15)</span>
                </InfoTip>
              </label>
              <input
                id="cfg-thumb-fps"
                type="number"
                step="0.2"
                value={draft.media.thumbnail_fps}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    media: {
                      ...draft.media,
                      thumbnail_fps: numVal(e.target.value, draft.media.thumbnail_fps),
                    },
                  })
                }
                className={inputCls}
              />
            </div>

            <div>
              <label htmlFor="cfg-clip-quota" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.clipQuotaGb} underline focusable>
                  <span>Clip Ring Quota GB (0.5–500)</span>
                </InfoTip>
              </label>
              <input
                id="cfg-clip-quota"
                type="number"
                step="0.5"
                value={draft.media.clip_quota_gb}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    media: {
                      ...draft.media,
                      clip_quota_gb: numVal(e.target.value, draft.media.clip_quota_gb),
                    },
                  })
                }
                className={inputCls}
              />
            </div>
          </div>
        </div>

        <div className="space-y-2.5 border-t border-[var(--border-hairline)] pt-4">
          <div>
            <h4 className="text-xs font-semibold text-[var(--text-secondary)]">
              <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.sectionExtraction} underline focusable>
                <span>Extraction, PO-Token & Outbound Network</span>
              </InfoTip>
            </h4>
            <p className="mt-0.5 text-[11px] text-[var(--text-muted)]/85">
              Controls how PGMWatch fetches stream URLs from YouTube, Twitch, Facebook, and TikTok, browser fallback, and multi-NIC binding.
            </p>
          </div>
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
            <div>
              <label htmlFor="cfg-auth-mode" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.authMode} underline focusable>
                  <span>Auth Mode</span>
                </InfoTip>
              </label>
              <select
                id="cfg-auth-mode"
                value={draft.extraction.auth_mode}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    extraction: { ...draft.extraction, auth_mode: e.target.value },
                  })
                }
                className={selectCls}
              >
                <option value="anonymous">anonymous (zero cookies)</option>
                <option value="fallback_auth">fallback_auth (managed Edge cookies)</option>
              </select>
            </div>

            <div>
              <label htmlFor="cfg-max-extractors" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.maxConcurrentExtractors} underline focusable>
                  <span>Max Concurrent Extractors (1–32)</span>
                </InfoTip>
              </label>
              <input
                id="cfg-max-extractors"
                type="number"
                step="1"
                value={draft.extraction.max_concurrent_extractors}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    extraction: {
                      ...draft.extraction,
                      max_concurrent_extractors: numVal(
                        e.target.value,
                        draft.extraction.max_concurrent_extractors
                      ),
                    },
                  })
                }
                className={inputCls}
              />
            </div>

            <div>
              <label htmlFor="cfg-pot-port" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.bgutilPotPort} underline focusable>
                  <span>bgutil PO-Token Port</span>
                </InfoTip>
              </label>
              <input
                id="cfg-pot-port"
                type="number"
                step="1"
                value={draft.extraction.bgutil_pot_port}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    extraction: {
                      ...draft.extraction,
                      bgutil_pot_port: numVal(
                        e.target.value,
                        draft.extraction.bgutil_pot_port
                      ),
                    },
                  })
                }
                className={inputCls}
              />
            </div>

            <div>
              <label htmlFor="cfg-outbound-ip" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.outboundBindIp} underline focusable>
                  <span>Outbound Bind IP (optional)</span>
                </InfoTip>
              </label>
              <input
                id="cfg-outbound-ip"
                type="text"
                placeholder="e.g. 192.168.1.50 (blank = default route)"
                value={draft.server.outbound_bind_ip}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    server: { ...draft.server, outbound_bind_ip: e.target.value },
                  })
                }
                className={inputCls}
              />
            </div>

            <div>
              <label htmlFor="cfg-msedge-path" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.msedgePath} underline focusable>
                  <span>Browser Path Override</span>
                </InfoTip>
              </label>
              <input
                id="cfg-msedge-path"
                type="text"
                placeholder="Auto-detect Edge / Chrome / Brave"
                value={draft.extraction.msedge_path}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    extraction: { ...draft.extraction, msedge_path: e.target.value },
                  })
                }
                className={inputCls}
              />
            </div>

            <div className="flex items-end pb-1.5">
              <label className="inline-flex cursor-pointer items-center gap-2 text-xs text-[var(--text-primary)]">
                <input
                  id="cfg-enable-pot"
                  type="checkbox"
                  checked={draft.extraction.enable_bgutil_pot}
                  onChange={(e) =>
                    setDraft({
                      ...draft,
                      extraction: {
                        ...draft.extraction,
                        enable_bgutil_pot: e.target.checked,
                      },
                    })
                  }
                  className="h-4 w-4 rounded border-[var(--border-hairline)] accent-emerald-500"
                />
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.enableBgutilPot} underline focusable>
                  <span>Enable bgutil PO-Token provider</span>
                </InfoTip>
              </label>
            </div>
          </div>
        </div>

        <div className="space-y-2.5 border-t border-[var(--border-hairline)] pt-4">
          <div>
            <h4 className="text-xs font-semibold text-[var(--text-secondary)]">
              <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.sectionGovernor} underline focusable>
                <span>Resource Governor & Alerts</span>
              </InfoTip>
            </h4>
            <p className="mt-0.5 text-[11px] text-[var(--text-muted)]/85">
              Auto-protects Master streams when internet/GPU/RAM gets tight, and tunes fault timing, loudness limits, and live-chat spike thresholds.
            </p>
          </div>
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
            <div>
              <label htmlFor="cfg-wan-cap" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.wanCapMbps} underline focusable>
                  <span>WAN Cap (Mbps)</span>
                </InfoTip>
              </label>
              <input
                id="cfg-wan-cap"
                type="number"
                step="10"
                value={draft.governor.wan_cap_mbps}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    governor: {
                      ...draft.governor,
                      wan_cap_mbps: numVal(e.target.value, draft.governor.wan_cap_mbps),
                    },
                  })
                }
                className={inputCls}
              />
            </div>

            <div>
              <label htmlFor="cfg-wan-admit" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.wanAdmitMaxPct} underline focusable>
                  <span>Bandwidth Limit %</span>
                </InfoTip>
              </label>
              <input
                id="cfg-wan-admit"
                type="number"
                step="1"
                value={draft.governor.wan_admit_max_pct}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    governor: {
                      ...draft.governor,
                      wan_admit_max_pct: numVal(
                        e.target.value,
                        draft.governor.wan_admit_max_pct
                      ),
                    },
                  })
                }
                className={inputCls}
              />
            </div>

            <div>
              <label htmlFor="cfg-vram-admit" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.vramAdmitMaxPct} underline focusable>
                  <span>GPU Memory Limit %</span>
                </InfoTip>
              </label>
              <input
                id="cfg-vram-admit"
                type="number"
                step="1"
                value={draft.governor.vram_admit_max_pct}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    governor: {
                      ...draft.governor,
                      vram_admit_max_pct: numVal(
                        e.target.value,
                        draft.governor.vram_admit_max_pct
                      ),
                    },
                  })
                }
                className={inputCls}
              />
            </div>

            <div>
              <label htmlFor="cfg-ram-admit" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.ramAdmitMaxPct} underline focusable>
                  <span>System RAM Limit %</span>
                </InfoTip>
              </label>
              <input
                id="cfg-ram-admit"
                type="number"
                step="1"
                value={draft.governor.ram_admit_max_pct}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    governor: {
                      ...draft.governor,
                      ram_admit_max_pct: numVal(
                        e.target.value,
                        draft.governor.ram_admit_max_pct
                      ),
                    },
                  })
                }
                className={inputCls}
              />
            </div>

            <div>
              <label htmlFor="cfg-worker-mem" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.workerMemoryLimitMb} underline focusable>
                  <span>Worker Mem Limit (MB)</span>
                </InfoTip>
              </label>
              <input
                id="cfg-worker-mem"
                type="number"
                step="64"
                value={draft.governor.worker_memory_limit_mb}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    governor: {
                      ...draft.governor,
                      worker_memory_limit_mb: numVal(
                        e.target.value,
                        draft.governor.worker_memory_limit_mb
                      ),
                    },
                  })
                }
                className={inputCls}
              />
            </div>

            <div>
              <label htmlFor="cfg-recovery-hyst" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.recoveryHysteresisSecs} underline focusable>
                  <span>Alert Clear Delay (s)</span>
                </InfoTip>
              </label>
              <input
                id="cfg-recovery-hyst"
                type="number"
                step="1"
                value={draft.alerts.recovery_hysteresis_secs}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    alerts: {
                      ...draft.alerts,
                      recovery_hysteresis_secs: numVal(
                        e.target.value,
                        draft.alerts.recovery_hysteresis_secs
                      ),
                    },
                  })
                }
                className={inputCls}
              />
            </div>

            <div>
              <label htmlFor="cfg-crit-esc" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.criticalEscalationSecs} underline focusable>
                  <span>Critical Escalation (s)</span>
                </InfoTip>
              </label>
              <input
                id="cfg-crit-esc"
                type="number"
                step="1"
                value={draft.alerts.critical_escalation_secs}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    alerts: {
                      ...draft.alerts,
                      critical_escalation_secs: numVal(
                        e.target.value,
                        draft.alerts.critical_escalation_secs
                      ),
                    },
                  })
                }
                className={inputCls}
              />
            </div>

            <div>
              <label htmlFor="cfg-chat-spike" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.chatSpikeWindow} underline focusable>
                  <span>Chat Spike Viewers / Window (s)</span>
                </InfoTip>
              </label>
              <div className="flex gap-1.5">
                <input
                  id="cfg-chat-spike"
                  aria-label="Chat spike unique viewers threshold"
                  type="number"
                  step="1"
                  value={draft.alerts.chat_spike_unique_viewers}
                  onChange={(e) =>
                    setDraft({
                      ...draft,
                      alerts: {
                        ...draft.alerts,
                        chat_spike_unique_viewers: numVal(
                          e.target.value,
                          draft.alerts.chat_spike_unique_viewers
                        ),
                      },
                    })
                  }
                  className={inputCls}
                />
                <input
                  id="cfg-chat-window"
                  aria-label="Chat spike window seconds"
                  type="number"
                  step="1"
                  value={draft.alerts.chat_spike_window_secs}
                  onChange={(e) =>
                    setDraft({
                      ...draft,
                      alerts: {
                        ...draft.alerts,
                        chat_spike_window_secs: numVal(
                          e.target.value,
                          draft.alerts.chat_spike_window_secs
                        ),
                      },
                    })
                  }
                  className={inputCls}
                />
              </div>
            </div>

            <div>
              <label htmlFor="cfg-custom-lufs" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.customTargetLufs} underline focusable>
                  <span>Default LUFS Target (blank = Platform Auto)</span>
                </InfoTip>
              </label>
              <input
                id="cfg-custom-lufs"
                type="text"
                inputMode="decimal"
                placeholder="Auto (-14 YT/TW, -13 FB, -11 TT)"
                value={customLufsText}
                onChange={(e) => {
                  const text = e.target.value;
                  setCustomLufsText(text);
                  const trimmed = text.trim();
                  if (trimmed === "") {
                    setDraft({
                      ...draft,
                      alerts: { ...draft.alerts, custom_target_lufs: null },
                    });
                    return;
                  }
                  if (trimmed === "-" || trimmed === "." || trimmed === "-.") return;
                  const parsed = Number(trimmed);
                  if (Number.isFinite(parsed)) {
                    setDraft({
                      ...draft,
                      alerts: { ...draft.alerts, custom_target_lufs: parsed },
                    });
                  }
                }}
                onBlur={() => {
                  setCustomLufsText(
                    draft.alerts.custom_target_lufs == null
                      ? ""
                      : String(draft.alerts.custom_target_lufs)
                  );
                }}
                className={inputCls}
              />
            </div>

            <div>
              <label htmlFor="cfg-lufs-tol" className={labelCls}>
                <InfoTip entry={RUNTIME_SETTINGS_GLOSSARY.lufsTolerance} underline focusable>
                  <span>LUFS Tolerance Window (± LU)</span>
                </InfoTip>
              </label>
              <input
                id="cfg-lufs-tol"
                type="number"
                step="0.5"
                min="0.5"
                max="10"
                value={draft.alerts.lufs_tolerance}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    alerts: {
                      ...draft.alerts,
                      lufs_tolerance: numVal(e.target.value, draft.alerts.lufs_tolerance),
                    },
                  })
                }
                className={inputCls}
              />
            </div>
          </div>
        </div>

        <div className="flex flex-wrap items-center justify-between gap-3 border-t border-[var(--border-hairline)] pt-4">
          <div className="text-xs text-[var(--text-muted)]">
            {localBanner ? (
              <span className={localBanner.isError ? "text-[var(--negative-text)]" : "text-[var(--positive-text)]"}>
                {localBanner.text}
              </span>
            ) : isDirty ? (
              <span className="text-[var(--warning-text)]">Unsaved runtime config changes</span>
            ) : (
              <span>Saved on disk in pgmwatch.toml</span>
            )}
          </div>
          <div className="flex items-center gap-2">
            {isDirty && saved && (
              <button
                type="button"
                onClick={() => {
                  setDraft(saved);
                  setCustomLufsText(
                    saved.alerts.custom_target_lufs == null
                      ? ""
                      : String(saved.alerts.custom_target_lufs)
                  );
                  setLocalBanner(null);
                }}
                className="h-9 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-3 text-xs font-medium text-[var(--text-primary)] hover:bg-[var(--bg-page)]"
              >
                Discard
              </button>
            )}
            <button
              type="submit"
              disabled={saving}
              className="inline-flex h-9 items-center gap-1.5 rounded-lg bg-[var(--text-primary)] px-4 text-xs font-semibold text-[var(--bg-rail)] disabled:opacity-50"
            >
              <Save className="h-3.5 w-3.5" />
              {saving ? "Saving..." : "Save Runtime Config"}
            </button>
          </div>
        </div>
      </form>
    </section>
  );
}
