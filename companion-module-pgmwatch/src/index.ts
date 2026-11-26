import {
  CompanionActionDefinitions,
  CompanionFeedbackDefinitions,
  CompanionPresetDefinitions,
  CompanionVariableDefinition,
  DropdownChoice,
  InstanceBase,
  InstanceStatus,
  runEntrypoint,
  SomeCompanionConfigField,
  combineRgb,
} from "@companion-module/base";
import WebSocket from "ws";
import { renderChannelVuPng64 } from "./png64";

interface Config {
  host: string;
  port: number;
}

interface SnapshotChannel {
  config: { id: string; group_id: string; name: string };
  alert_state: string;
  audio: {
    integrated_lufs: number;
    short_term_lufs: number;
    left_rms_dbfs: number;
    right_rms_dbfs: number;
  };
  ccv: { value: number | null; status: string };
  pfl_active: boolean;
}

interface SnapshotPayload {
  show: { groups: { id: string; name: string; language_code: string; intermission_active: boolean }[] };
  global_intermission: boolean;
  active_match: { label: string } | null;
  combined_ccv: number;
  combined_pcv: number;
  channels: SnapshotChannel[];
}

function tallyRgbForState(state: string): [number, number, number] {
  switch (state) {
    case "CRITICAL":
      return [229, 72, 77];
    case "WARNING":
      return [229, 169, 59];
    case "ACKNOWLEDGED":
      return [158, 53, 58];
    case "AD_BREAK":
      return [59, 130, 246];
    case "LOGIN_GATED":
    case "EXTRACTOR_ERR":
      return [167, 139, 250];
    case "HEALTHY":
      return [76, 195, 138];
    default:
      return [133, 133, 139];
  }
}

function normalizeHost(raw: string | undefined): string {
  const trimmed = (raw ?? "127.0.0.1").trim();
  if (!trimmed) return "127.0.0.1";
  try {
    const parsed = new URL(trimmed.includes("://") ? trimmed : `http://${trimmed}`);
    if (parsed.hostname) return parsed.hostname;
  } catch {
    // Fall back to basic sanitization if URL parsing fails
  }
  const cleaned = trimmed
    .replace(/^wss?:\/\//i, "")
    .replace(/^https?:\/\//i, "")
    .replace(/\/.*$/, "")
    .replace(/:\d+$/, "");
  return cleaned || "127.0.0.1";
}

function fmtDb(value: unknown): string {
  return typeof value === "number" && Number.isFinite(value) ? value.toFixed(1) : "--.-";
}

function dbfsToPct(dbfs: number): number {
  if (!Number.isFinite(dbfs)) return 0;
  return Math.round(Math.max(0, Math.min(100, ((dbfs + 60) / 60) * 100)));
}

function formatMeterRow(label: string, dbfs: number): string {
  const pct = dbfsToPct(dbfs);
  const filled = Math.round((pct / 100) * 7);
  return `${label} ${"▮".repeat(filled)}${"▯".repeat(7 - filled)}`;
}

function formatVuTextBar(leftDbfs: number, rightDbfs: number): string {
  return `${formatMeterRow("L", leftDbfs)}\n${formatMeterRow("R", rightDbfs)}`;
}

class PgmWatchCompanion extends InstanceBase<Config> {
  private ws: WebSocket | null = null;
  private reconnectTimer: NodeJS.Timeout | null = null;
  private meterTimer: NodeJS.Timeout | null = null;
  private lastMeterCheckAt = 0;
  private pendingMeterChannels = new Set<string>();
  private subscribedPngFeedbacks = new Set<string>();
  private snapshot: SnapshotPayload | null = null;
  private lastStructureKey = "";
  private config: Config = { host: "127.0.0.1", port: 8787 };

  async init(config: Config): Promise<void> {
    this.config = config;
    this.initActions();
    this.initFeedbacks();
    this.initVariables();
    this.connectWs();
  }

  async destroy(): Promise<void> {
    this.clearTimers();
    const oldWs = this.ws;
    this.ws = null;
    oldWs?.close();
  }

  async configUpdated(config: Config): Promise<void> {
    this.config = config;
    this.snapshot = null;
    this.lastStructureKey = "";
    this.clearTimers();
    const oldWs = this.ws;
    this.ws = null;
    oldWs?.close();
    this.connectWs();
  }

  getConfigFields(): SomeCompanionConfigField[] {
    return [
      { type: "textinput", id: "host", label: "PGMWatch Host", width: 8, default: "127.0.0.1" },
      { type: "number", id: "port", label: "Port", width: 4, default: 8787, min: 1, max: 65535 },
    ];
  }

  private clearTimers(): void {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.meterTimer) {
      clearTimeout(this.meterTimer);
      this.meterTimer = null;
    }
    this.pendingMeterChannels.clear();
  }

  private connectWs(): void {
    this.clearTimers();
    const host = normalizeHost(this.config.host);
    const port = Number(this.config.port) || 8787;
    const url = `ws://${host}:${port}/ws`;
    this.updateStatus(InstanceStatus.Connecting);
    let ws: WebSocket;
    try {
      ws = new WebSocket(url);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      this.log("error", `Invalid PGMWatch WebSocket URL (${url}): ${msg}`);
      this.updateStatus(InstanceStatus.BadConfig, `Invalid host/port: ${msg}`);
      return;
    }
    this.ws = ws;

    ws.on("open", () => {
      if (this.ws !== ws) return;
      this.updateStatus(InstanceStatus.Ok);
      ws.send(JSON.stringify({ cmd: "request_snapshot" }));
    });

    ws.on("message", (raw) => {
      if (this.ws !== ws) return;
      try {
        const msg = JSON.parse(raw.toString());
        if (msg.event === "snapshot" && msg.snapshot) {
          this.snapshot = msg.snapshot;
          this.syncStateFromSnapshot();
        } else if (msg.event === "meter" && msg.channel_id && msg.audio && this.snapshot) {
          const ch = this.snapshot.channels.find((c) => c.config.id === msg.channel_id);
          if (ch) {
            ch.audio = msg.audio;
            this.pendingMeterChannels.add(ch.config.id);
            this.scheduleMeterFeedback();
          }
        }
      } catch {
        // Ignore malformed payloads
      }
    });

    ws.on("close", () => {
      if (this.ws !== ws) return;
      this.ws = null;
      this.updateStatus(InstanceStatus.Disconnected);
      this.reconnectTimer = setTimeout(() => this.connectWs(), 3000);
    });

    ws.on("error", (err) => {
      if (this.ws !== ws) return;
      this.log("warn", `PGMWatch WebSocket error: ${err.message}`);
      this.updateStatus(InstanceStatus.ConnectionFailure, err.message);
    });
  }

  private flushMeterUpdates(): void {
    this.lastMeterCheckAt = Date.now();
    try {
      if (this.snapshot && this.pendingMeterChannels.size > 0) {
        const updates: Record<string, string | number> = {};
        for (const cid of this.pendingMeterChannels) {
          const ch = this.snapshot.channels.find((c) => c.config.id === cid);
          if (!ch || !ch.audio) continue;
          Object.assign(updates, this.buildChannelAudioVars(ch));
        }
        this.pendingMeterChannels.clear();
        if (Object.keys(updates).length > 0) {
          this.setVariableValues(updates);
        }
      }
      if (this.subscribedPngFeedbacks.size > 0) {
        this.checkFeedbacks("channel_vu_png");
      }
    } catch (err) {
      this.log("warn", `Failed updating PGMWatch meter variables: ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  private buildChannelAudioVars(ch: SnapshotChannel): Record<string, string | number> {
    const cid = ch.config.id;
    const avgLufs = ch.audio?.integrated_lufs ?? ch.audio?.short_term_lufs;
    const leftDb = ch.audio?.left_rms_dbfs;
    const rightDb = ch.audio?.right_rms_dbfs;
    return {
      [`ch_${cid}_lufs`]: fmtDb(avgLufs),
      [`ch_${cid}_left_db`]: fmtDb(leftDb),
      [`ch_${cid}_right_db`]: fmtDb(rightDb),
      [`ch_${cid}_left_pct`]: dbfsToPct(leftDb),
      [`ch_${cid}_right_pct`]: dbfsToPct(rightDb),
      [`ch_${cid}_vu_bar`]: formatVuTextBar(leftDb, rightDb),
    };
  }

  private scheduleMeterFeedback(): void {
    const now = Date.now();
    const elapsed = now - this.lastMeterCheckAt;
    if (elapsed >= 90) {
      this.flushMeterUpdates();
      return;
    }
    if (!this.meterTimer) {
      this.meterTimer = setTimeout(() => {
        this.meterTimer = null;
        this.flushMeterUpdates();
      }, 90 - elapsed);
    }
  }

  private sendCmd(payload: Record<string, unknown>): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(payload));
    }
  }

  private getChannelChoices(): DropdownChoice[] {
    const channels = this.snapshot?.channels ?? [];
    if (channels.length === 0) {
      return [{ id: "", label: "(Connect to PGMWatch to load channels)" }];
    }
    return channels.map((c) => ({
      id: c.config.id,
      label: `${c.config.name} (${c.config.group_id})`,
    }));
  }

  private getGroupChoices(includeGlobal: boolean): DropdownChoice[] {
    const groups = this.snapshot?.show.groups ?? [];
    const items: DropdownChoice[] = includeGlobal
      ? [{ id: "", label: "Global (All Groups)" }]
      : [];
    for (const g of groups) {
      items.push({ id: g.id, label: `${g.language_code} — ${g.name}` });
    }
    if (items.length === 0) {
      items.push({ id: "", label: "(Connect to PGMWatch to load groups)" });
    }
    return items;
  }

  private initActions(): void {
    const chChoices = this.getChannelChoices();
    const defaultCh = chChoices[0]?.id ?? "";
    const grpWithGlobal = this.getGroupChoices(true);
    const channelOrAllChoices: DropdownChoice[] = [
      { id: "", label: "All Channels" },
      ...(this.snapshot?.channels ?? []).map((c) => ({
        id: c.config.id,
        label: `${c.config.name} (${c.config.group_id})`,
      })),
    ];

    const actions: CompanionActionDefinitions = {
      ack_channel: {
        name: "Acknowledge Channel Fault",
        options: [
          {
            type: "dropdown",
            id: "channel_id",
            label: "Channel",
            choices: chChoices,
            default: defaultCh,
            allowCustom: true,
          },
        ],
        callback: (ev) => {
          const cid = String(ev.options.channel_id ?? "").trim();
          if (cid) this.sendCmd({ cmd: "ack", channel_id: cid });
        },
      },
      force_clip: {
        name: "Force 5s Evidence Clip",
        options: [
          {
            type: "dropdown",
            id: "channel_id",
            label: "Channel",
            choices: chChoices,
            default: defaultCh,
            allowCustom: true,
          },
        ],
        callback: (ev) => {
          const cid = String(ev.options.channel_id ?? "").trim();
          if (cid) this.sendCmd({ cmd: "force_clip", channel_id: cid });
        },
      },
      toggle_pfl: {
        name: "Toggle Listen (PFL Audio)",
        options: [
          {
            type: "dropdown",
            id: "channel_id",
            label: "Channel",
            choices: chChoices,
            default: defaultCh,
            allowCustom: true,
          },
        ],
        callback: (ev) => {
          const cid = String(ev.options.channel_id ?? "").trim();
          if (cid) this.sendCmd({ cmd: "toggle_pfl", channel_id: cid });
        },
      },
      toggle_intermission: {
        name: "Hold Mode (Global or Group)",
        options: [
          {
            type: "dropdown",
            id: "group_id",
            label: "Scope",
            choices: grpWithGlobal,
            default: "",
            allowCustom: true,
          },
          {
            type: "dropdown",
            id: "mode",
            label: "Action",
            choices: [
              { id: "toggle", label: "Toggle" },
              { id: "on", label: "Enable Hold" },
              { id: "off", label: "Resume (End Hold)" },
            ],
            default: "toggle",
          },
        ],
        callback: (ev) => {
          const gid = String(ev.options.group_id ?? "").trim();
          const mode = ev.options.mode ? String(ev.options.mode) : undefined;
          let nextActive: boolean;
          if (mode === "on") {
            nextActive = true;
          } else if (mode === "off") {
            nextActive = false;
          } else if (mode === "toggle") {
            if (gid) {
              const grp = this.snapshot?.show.groups.find((g) => g.id === gid);
              nextActive = !(grp?.intermission_active ?? false);
            } else {
              nextActive = !(this.snapshot?.global_intermission ?? false);
            }
          } else {
            nextActive = Boolean(ev.options.active ?? true);
          }
          this.sendCmd({
            cmd: "set_intermission",
            group_id: gid ? gid : null,
            active: nextActive,
          });
        },
      },
      start_match: {
        name: "Start Segment Marker",
        options: [{ type: "textinput", id: "label", label: "Segment Label", default: "Segment 1" }],
        callback: (ev) =>
          this.sendCmd({ cmd: "start_match", label: String(ev.options.label || "Segment") }),
      },
      end_match: {
        name: "End Active Segment Marker",
        options: [],
        callback: () => this.sendCmd({ cmd: "end_match" }),
      },
      reset_lufs: {
        name: "Reset Channel Avg Loudness (LUFS)",
        options: [
          {
            type: "dropdown",
            id: "channel_id",
            label: "Channel",
            choices: channelOrAllChoices,
            default: "",
            allowCustom: true,
          },
        ],
        callback: (ev) => {
          const cid = String(ev.options.channel_id ?? "").trim();
          this.sendCmd({ cmd: "reset_lufs", channel_id: cid ? cid : null });
        },
      },
      reset_all_lufs: {
        name: "Reset All Avg Loudness (LUFS)",
        options: [],
        callback: () => this.sendCmd({ cmd: "reset_lufs", channel_id: null }),
      },
    };
    this.setActionDefinitions(actions);
  }

  private initFeedbacks(): void {
    const chChoices = this.getChannelChoices();
    const defaultCh = chChoices[0]?.id ?? "";
    const grpChoices = this.getGroupChoices(false);
    const defaultGrp = grpChoices[0]?.id ?? "";

    const feedbacks: CompanionFeedbackDefinitions = {
      channel_tally: {
        type: "advanced",
        name: "Channel Tally Color",
        options: [
          {
            type: "dropdown",
            id: "channel_id",
            label: "Channel",
            choices: chChoices,
            default: defaultCh,
            allowCustom: true,
          },
        ],
        callback: (fb) => {
          const ch = this.snapshot?.channels.find((c) => c.config.id === fb.options.channel_id);
          const [r, g, b] = tallyRgbForState(ch?.alert_state || "OFFLINE");
          return { bgcolor: combineRgb(r, g, b), color: combineRgb(15, 15, 16) };
        },
      },
      channel_vu_png: {
        type: "advanced",
        name: "Channel Live Stereo VU png64",
        options: [
          {
            type: "dropdown",
            id: "channel_id",
            label: "Channel",
            choices: chChoices,
            default: defaultCh,
            allowCustom: true,
          },
        ],
        subscribe: (fb) => {
          this.subscribedPngFeedbacks.add(fb.id);
        },
        unsubscribe: (fb) => {
          this.subscribedPngFeedbacks.delete(fb.id);
        },
        callback: (fb) => {
          const ch = this.snapshot?.channels.find((c) => c.config.id === fb.options.channel_id);
          if (!ch) return {};
          const png64 = renderChannelVuPng64(
            ch.audio.left_rms_dbfs,
            ch.audio.right_rms_dbfs,
            tallyRgbForState(ch.alert_state)
          );
          return { png64 };
        },
      },
      group_rollup: {
        type: "advanced",
        name: "Language Group Roll-Up Tally",
        options: [
          {
            type: "dropdown",
            id: "group_id",
            label: "Group",
            choices: grpChoices,
            default: defaultGrp,
            allowCustom: true,
          },
        ],
        callback: (fb) => {
          const grp = this.snapshot?.show.groups.find((g) => g.id === fb.options.group_id);
          if (grp?.intermission_active) {
            return { bgcolor: combineRgb(229, 169, 59), color: combineRgb(15, 15, 16) };
          }
          const members =
            this.snapshot?.channels.filter((c) => c.config.group_id === fb.options.group_id) || [];
          const states = members.map((m) => m.alert_state);
          const worst = states.includes("CRITICAL")
            ? "CRITICAL"
            : states.includes("WARNING")
            ? "WARNING"
            : states.includes("HEALTHY")
            ? "HEALTHY"
            : "OFFLINE";
          const [r, g, b] = tallyRgbForState(worst);
          return { bgcolor: combineRgb(r, g, b), color: combineRgb(15, 15, 16) };
        },
      },
      global_hold: {
        type: "boolean",
        name: "Global Hold Mode Active",
        defaultStyle: {
          bgcolor: combineRgb(229, 169, 59),
          color: combineRgb(15, 15, 16),
          text: "HOLD\nACTIVE",
        },
        options: [],
        callback: () => Boolean(this.snapshot?.global_intermission),
      },
      channel_pfl: {
        type: "boolean",
        name: "Channel Listen (PFL) Active",
        defaultStyle: {
          bgcolor: combineRgb(59, 130, 246),
          color: combineRgb(255, 255, 255),
        },
        options: [
          {
            type: "dropdown",
            id: "channel_id",
            label: "Channel",
            choices: chChoices,
            default: defaultCh,
            allowCustom: true,
          },
        ],
        callback: (fb) =>
          Boolean(
            this.snapshot?.channels.find((c) => c.config.id === fb.options.channel_id)?.pfl_active
          ),
      },
    };
    this.setFeedbackDefinitions(feedbacks);
  }

  private initVariables(): void {
    const vars: CompanionVariableDefinition[] = [
      { variableId: "combined_ccv", name: "Live Viewers (Combined)" },
      { variableId: "combined_pcv", name: "Peak Viewers (Combined)" },
      { variableId: "active_match", name: "Active Segment" },
      { variableId: "global_hold", name: "Global Hold Mode" },
    ];
    this.setVariableDefinitions(vars);
  }

  private syncStateFromSnapshot(): void {
    if (!this.snapshot) return;

    const structureKey = JSON.stringify({
      g: this.snapshot.show.groups.map((g) => [g.id, g.name, g.language_code]),
      c: this.snapshot.channels.map((c) => [c.config.id, c.config.group_id, c.config.name]),
    });
    const structureChanged = structureKey !== this.lastStructureKey;

    if (structureChanged) {
      this.lastStructureKey = structureKey;
      this.initActions();
      this.initFeedbacks();

      const vars: CompanionVariableDefinition[] = [
        { variableId: "combined_ccv", name: "Live Viewers (Combined)" },
        { variableId: "combined_pcv", name: "Peak Viewers (Combined)" },
        { variableId: "active_match", name: "Active Segment" },
        { variableId: "global_hold", name: "Global Hold Mode" },
      ];
      for (const grp of this.snapshot.show.groups) {
        vars.push({ variableId: `group_${grp.id}_ccv`, name: `${grp.name} Live Viewers` });
      }
      for (const ch of this.snapshot.channels) {
        const cid = ch.config.id;
        vars.push({ variableId: `ch_${cid}_lufs`, name: `${ch.config.name} Avg Loudness (LUFS)` });
        vars.push({ variableId: `ch_${cid}_left_db`, name: `${ch.config.name} Left Level (dBFS)` });
        vars.push({ variableId: `ch_${cid}_right_db`, name: `${ch.config.name} Right Level (dBFS)` });
        vars.push({ variableId: `ch_${cid}_left_pct`, name: `${ch.config.name} Left Meter % (0-100)` });
        vars.push({ variableId: `ch_${cid}_right_pct`, name: `${ch.config.name} Right Meter % (0-100)` });
        vars.push({ variableId: `ch_${cid}_vu_bar`, name: `${ch.config.name} Text VU Bar` });
        vars.push({ variableId: `ch_${cid}_state`, name: `${ch.config.name} Alert State` });
      }
      this.setVariableDefinitions(vars);
      this.rebuildPresets();
    }

    const values: Record<string, string | number> = {
      combined_ccv: this.snapshot.combined_ccv,
      combined_pcv: this.snapshot.combined_pcv,
      active_match: this.snapshot.active_match?.label ?? "None",
      global_hold: this.snapshot.global_intermission ? "ACTIVE" : "OFF",
    };
    for (const grp of this.snapshot.show.groups) {
      const groupCcv = this.snapshot.channels
        .filter((c) => c.config.group_id === grp.id && c.ccv.status === "VALUE" && c.ccv.value !== null)
        .reduce((acc, c) => acc + (c.ccv.value || 0), 0);
      values[`group_${grp.id}_ccv`] = groupCcv;
    }
    for (const ch of this.snapshot.channels) {
      Object.assign(values, this.buildChannelAudioVars(ch));
      values[`ch_${ch.config.id}_state`] = ch.alert_state;
    }
    this.setVariableValues(values);
    this.checkFeedbacks("channel_tally", "channel_vu_png", "group_rollup", "global_hold", "channel_pfl");
  }

  private rebuildPresets(): void {
    if (!this.snapshot) return;
    const label = this.label || "PGMWatch";
    const presets: CompanionPresetDefinitions = {};

    for (const grp of this.snapshot.show.groups) {
      const gid = grp.id;
      presets[`overview_group_${gid}`] = {
        type: "button",
        category: "Overview — Language Groups",
        name: grp.name,
        style: {
          text: `${grp.language_code}\n$(${label}:group_${gid}_ccv)`,
          size: "14",
          color: combineRgb(237, 237, 237),
          bgcolor: combineRgb(25, 25, 27),
        },
        steps: [
          {
            down: [{ actionId: "toggle_intermission", options: { group_id: gid, mode: "toggle" } }],
            up: [],
          },
        ],
        feedbacks: [{ feedbackId: "group_rollup", options: { group_id: gid } }],
      };
    }

    presets["overview_global_hold"] = {
      type: "button",
      category: "Overview — Controls",
      name: "Global Hold Mode",
      style: {
        text: "HOLD\nMODE",
        size: "14",
        color: combineRgb(237, 237, 237),
        bgcolor: combineRgb(35, 35, 37),
      },
      steps: [
        {
          down: [{ actionId: "toggle_intermission", options: { group_id: "", mode: "toggle" } }],
          up: [],
        },
      ],
      feedbacks: [
        {
          feedbackId: "global_hold",
          options: {},
          style: {
            bgcolor: combineRgb(229, 169, 59),
            color: combineRgb(15, 15, 16),
            text: "HOLD\nACTIVE",
          },
        },
      ],
    };

    presets["overview_reset_all_lufs"] = {
      type: "button",
      category: "Overview — Controls",
      name: "Reset All Avg Loudness",
      style: {
        text: "RESET ALL\nLOUDNESS",
        size: "14",
        color: combineRgb(237, 237, 237),
        bgcolor: combineRgb(35, 35, 37),
      },
      steps: [{ down: [{ actionId: "reset_all_lufs", options: {} }], up: [] }],
      feedbacks: [],
    };

    presets["overview_start_segment"] = {
      type: "button",
      category: "Overview — Controls",
      name: "Start Segment",
      style: {
        text: "START\nSEGMENT",
        size: "14",
        color: combineRgb(237, 237, 237),
        bgcolor: combineRgb(35, 35, 37),
      },
      steps: [{ down: [{ actionId: "start_match", options: { label: "Segment 1" } }], up: [] }],
      feedbacks: [],
    };

    presets["overview_end_segment"] = {
      type: "button",
      category: "Overview — Controls",
      name: "End Active Segment",
      style: {
        text: `END\n$(${label}:active_match)`,
        size: "14",
        color: combineRgb(237, 237, 237),
        bgcolor: combineRgb(35, 35, 37),
      },
      steps: [{ down: [{ actionId: "end_match", options: {} }], up: [] }],
      feedbacks: [],
    };

    for (const ch of this.snapshot.channels) {
      const cid = ch.config.id;
      presets[`detail_vu_${cid}`] = {
        type: "button",
        category: `Group Detail — ${ch.config.group_id}`,
        name: `${ch.config.name} VU/Listen (Text)`,
        style: {
          text: `${ch.config.name}\n$(${label}:ch_${cid}_vu_bar)\n$(${label}:ch_${cid}_lufs) LUFS`,
          size: "7",
          color: combineRgb(237, 237, 237),
          bgcolor: combineRgb(20, 20, 21),
        },
        steps: [{ down: [{ actionId: "toggle_pfl", options: { channel_id: cid } }], up: [] }],
        feedbacks: [
          { feedbackId: "channel_tally", options: { channel_id: cid } },
          {
            feedbackId: "channel_pfl",
            options: { channel_id: cid },
            style: { bgcolor: combineRgb(37, 99, 235), color: combineRgb(255, 255, 255) },
          },
        ],
      };

      presets[`detail_vu_png_${cid}`] = {
        type: "button",
        category: `Group Detail — ${ch.config.group_id}`,
        name: `${ch.config.name} VU/Listen (Image)`,
        style: {
          text: `${ch.config.name}\n$(${label}:ch_${cid}_lufs) LUFS`,
          size: "7",
          color: combineRgb(237, 237, 237),
          bgcolor: combineRgb(20, 20, 21),
        },
        steps: [{ down: [{ actionId: "toggle_pfl", options: { channel_id: cid } }], up: [] }],
        feedbacks: [
          { feedbackId: "channel_vu_png", options: { channel_id: cid } },
          {
            feedbackId: "channel_pfl",
            options: { channel_id: cid },
            style: { color: combineRgb(147, 197, 253) },
          },
        ],
      };

      presets[`detail_ack_${cid}`] = {
        type: "button",
        category: `Group Detail — ${ch.config.group_id}`,
        name: `${ch.config.name} Ack`,
        style: {
          text: `ACK\n${ch.config.name}`,
          size: "14",
          color: combineRgb(237, 237, 237),
          bgcolor: combineRgb(35, 35, 37),
        },
        steps: [{ down: [{ actionId: "ack_channel", options: { channel_id: cid } }], up: [] }],
        feedbacks: [{ feedbackId: "channel_tally", options: { channel_id: cid } }],
      };

      presets[`detail_clip_${cid}`] = {
        type: "button",
        category: `Group Detail — ${ch.config.group_id}`,
        name: `${ch.config.name} Clip`,
        style: {
          text: `5s CLIP\n${ch.config.name}`,
          size: "14",
          color: combineRgb(237, 237, 237),
          bgcolor: combineRgb(35, 35, 37),
        },
        steps: [{ down: [{ actionId: "force_clip", options: { channel_id: cid } }], up: [] }],
        feedbacks: [],
      };

      presets[`detail_reset_lufs_${cid}`] = {
        type: "button",
        category: `Group Detail — ${ch.config.group_id}`,
        name: `${ch.config.name} Reset Loudness`,
        style: {
          text: `RST LUFS\n${ch.config.name}`,
          size: "14",
          color: combineRgb(237, 237, 237),
          bgcolor: combineRgb(35, 35, 37),
        },
        steps: [{ down: [{ actionId: "reset_lufs", options: { channel_id: cid } }], up: [] }],
        feedbacks: [],
      };
    }

    this.setPresetDefinitions(presets);
  }
}

runEntrypoint(PgmWatchCompanion, []);
