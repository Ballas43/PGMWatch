import {
  CompanionActionDefinitions,
  CompanionFeedbackDefinitions,
  CompanionPresetDefinitions,
  CompanionVariableDefinition,
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

class PgmWatchCompanion extends InstanceBase<Config> {
  private ws: WebSocket | null = null;
  private reconnectTimer: NodeJS.Timeout | null = null;
  private snapshot: SnapshotPayload | null = null;
  private config: Config = { host: "127.0.0.1", port: 8787 };

  async init(config: Config): Promise<void> {
    this.config = config;
    this.initActions();
    this.initFeedbacks();
    this.initVariables();
    this.connectWs();
  }

  async destroy(): Promise<void> {
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.ws?.close();
  }

  async configUpdated(config: Config): Promise<void> {
    this.config = config;
    this.ws?.close();
    this.connectWs();
  }

  getConfigFields(): SomeCompanionConfigField[] {
    return [
      { type: "textinput", id: "host", label: "PGMWatch Host", width: 8, default: "127.0.0.1" },
      { type: "number", id: "port", label: "Port", width: 4, default: 8787, min: 1, max: 65535 },
    ];
  }

  private connectWs(): void {
    const url = `ws://${this.config.host || "127.0.0.1"}:${this.config.port || 8787}/ws`;
    this.updateStatus(InstanceStatus.Connecting);
    const ws = new WebSocket(url);
    this.ws = ws;

    ws.on("open", () => {
      this.updateStatus(InstanceStatus.Ok);
      ws.send(JSON.stringify({ cmd: "request_snapshot" }));
    });

    ws.on("message", (raw) => {
      try {
        const msg = JSON.parse(raw.toString());
        if (msg.event === "snapshot" && msg.snapshot) {
          this.snapshot = msg.snapshot;
          this.updateVariablesAndPresets();
          this.checkFeedbacks("channel_tally", "channel_vu_png", "group_rollup");
        }
      } catch {}
    });

    ws.on("close", () => {
      this.updateStatus(InstanceStatus.Disconnected);
      this.reconnectTimer = setTimeout(() => this.connectWs(), 3000);
    });

    ws.on("error", () => {
      this.updateStatus(InstanceStatus.ConnectionFailure);
    });
  }

  private sendCmd(payload: Record<string, unknown>): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(payload));
    }
  }

  private initActions(): void {
    const actions: CompanionActionDefinitions = {
      ack_channel: {
        name: "Acknowledge Channel Fault",
        options: [{ type: "textinput", id: "channel_id", label: "Channel ID", default: "" }],
        callback: (ev) => this.sendCmd({ cmd: "ack", channel_id: ev.options.channel_id }),
      },
      force_clip: {
        name: "Force 5s Evidence Clip",
        options: [{ type: "textinput", id: "channel_id", label: "Channel ID", default: "" }],
        callback: (ev) => this.sendCmd({ cmd: "force_clip", channel_id: ev.options.channel_id }),
      },
      toggle_pfl: {
        name: "Toggle PFL Audio Listen",
        options: [{ type: "textinput", id: "channel_id", label: "Channel ID", default: "" }],
        callback: (ev) => this.sendCmd({ cmd: "toggle_pfl", channel_id: ev.options.channel_id }),
      },
      toggle_intermission: {
        name: "Toggle Intermission Mode",
        options: [
          { type: "textinput", id: "group_id", label: "Group ID (blank = Global)", default: "" },
          { type: "checkbox", id: "active", label: "Active", default: true },
        ],
        callback: (ev) =>
          this.sendCmd({
            cmd: "set_intermission",
            group_id: ev.options.group_id ? String(ev.options.group_id) : null,
            active: Boolean(ev.options.active),
          }),
      },
      start_match: {
        name: "Start Match Marker",
        options: [{ type: "textinput", id: "label", label: "Match Label", default: "Match 1" }],
        callback: (ev) => this.sendCmd({ cmd: "start_match", label: String(ev.options.label || "Match") }),
      },
      end_match: {
        name: "End Active Match Marker",
        options: [],
        callback: () => this.sendCmd({ cmd: "end_match" }),
      },
      reset_lufs: {
        name: "Reset Channel Avg LUFS",
        options: [{ type: "textinput", id: "channel_id", label: "Channel ID (blank = all)", default: "" }],
        callback: (ev) => {
          const cid = String(ev.options.channel_id ?? "").trim();
          this.sendCmd({ cmd: "reset_lufs", channel_id: cid ? cid : null });
        },
      },
      reset_all_lufs: {
        name: "Reset All Avg LUFS",
        options: [],
        callback: () => this.sendCmd({ cmd: "reset_lufs", channel_id: null }),
      },
    };
    this.setActionDefinitions(actions);
  }

  private initFeedbacks(): void {
    const feedbacks: CompanionFeedbackDefinitions = {
      channel_tally: {
        type: "advanced",
        name: "Channel Tally Color",
        options: [{ type: "textinput", id: "channel_id", label: "Channel ID", default: "" }],
        callback: (fb) => {
          const ch = this.snapshot?.channels.find((c) => c.config.id === fb.options.channel_id);
          const [r, g, b] = tallyRgbForState(ch?.alert_state || "OFFLINE");
          return { bgcolor: combineRgb(r, g, b), color: combineRgb(15, 15, 16) };
        },
      },
      channel_vu_png: {
        type: "advanced",
        name: "Channel Live Stereo VU png64",
        options: [{ type: "textinput", id: "channel_id", label: "Channel ID", default: "" }],
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
        options: [{ type: "textinput", id: "group_id", label: "Group ID", default: "" }],
        callback: (fb) => {
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
    };
    this.setFeedbackDefinitions(feedbacks);
  }

  private initVariables(): void {
    const vars: CompanionVariableDefinition[] = [
      { variableId: "combined_ccv", name: "Combined CCV" },
      { variableId: "combined_pcv", name: "Combined PCV" },
      { variableId: "active_match", name: "Active Match" },
    ];
    this.setVariableDefinitions(vars);
  }

  private updateVariablesAndPresets(): void {
    if (!this.snapshot) return;
    const vars: CompanionVariableDefinition[] = [
      { variableId: "combined_ccv", name: "Combined CCV" },
      { variableId: "combined_pcv", name: "Combined PCV" },
      { variableId: "active_match", name: "Active Match" },
    ];
    const values: Record<string, string | number> = {
      combined_ccv: this.snapshot.combined_ccv,
      combined_pcv: this.snapshot.combined_pcv,
      active_match: this.snapshot.active_match?.label ?? "None",
    };

    const presets: CompanionPresetDefinitions = {};

    // Level 1: Overview Page (Language Group Roll-ups + Match Markers + Intermission)
    for (const grp of this.snapshot.show.groups) {
      const gid = grp.id;
      const groupCcv = this.snapshot.channels
        .filter((c) => c.config.group_id === gid && c.ccv.status === "VALUE" && c.ccv.value !== null)
        .reduce((acc, c) => acc + (c.ccv.value || 0), 0);
      vars.push({ variableId: `group_${gid}_ccv`, name: `${grp.name} CCV` });
      values[`group_${gid}_ccv`] = groupCcv;

      presets[`overview_group_${gid}`] = {
        type: "button",
        category: "Overview — Language Groups",
        name: grp.name,
        style: {
          text: `${grp.language_code}\n$(pgmwatch:group_${gid}_ccv)`,
          size: "14",
          color: combineRgb(237, 237, 237),
          bgcolor: combineRgb(25, 25, 27),
        },
        steps: [
          {
            down: [
              {
                actionId: "toggle_intermission",
                options: { group_id: gid, active: !grp.intermission_active },
              },
            ],
            up: [],
          },
        ],
        feedbacks: [{ feedbackId: "group_rollup", options: { group_id: gid } }],
      };
    }

    presets["overview_reset_all_lufs"] = {
      type: "button",
      category: "Overview — Audio",
      name: "Reset All Avg LUFS",
      style: {
        text: "RESET ALL\nAVG LUFS",
        size: "14",
        color: combineRgb(237, 237, 237),
        bgcolor: combineRgb(35, 35, 37),
      },
      steps: [{ down: [{ actionId: "reset_all_lufs", options: {} }], up: [] }],
      feedbacks: [],
    };

    // Level 2: Group Detail Page (Live png64 VU/Avg LUFS, Ack, Clip, Reset LUFS, PFL)
    for (const ch of this.snapshot.channels) {
      const cid = ch.config.id;
      const avgLufs = ch.audio.integrated_lufs ?? ch.audio.short_term_lufs;
      vars.push({ variableId: `ch_${cid}_lufs`, name: `${ch.config.name} Avg LUFS` });
      values[`ch_${cid}_lufs`] = avgLufs.toFixed(1);

      presets[`detail_vu_${cid}`] = {
        type: "button",
        category: `Group Detail — ${ch.config.group_id}`,
        name: `${ch.config.name} VU/PFL`,
        style: {
          text: `${ch.config.name}\n$(pgmwatch:ch_${cid}_lufs) Avg LUFS`,
          size: "7",
          color: combineRgb(237, 237, 237),
          bgcolor: combineRgb(20, 20, 21),
        },
        steps: [
          { down: [{ actionId: "toggle_pfl", options: { channel_id: cid } }], up: [] },
        ],
        feedbacks: [{ feedbackId: "channel_vu_png", options: { channel_id: cid } }],
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
        name: `${ch.config.name} Reset Avg LUFS`,
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

    this.setVariableDefinitions(vars);
    this.setVariableValues(values);
    this.setPresetDefinitions(presets);
  }
}

runEntrypoint(PgmWatchCompanion, []);
