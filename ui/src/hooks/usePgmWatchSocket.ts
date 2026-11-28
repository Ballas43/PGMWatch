import { useCallback, useEffect, useRef, useState } from "react";
import { DashboardSnapshot } from "../types";
import { liveTelemetryStore } from "../utils/liveTelemetryStore";

export interface PflPacketHandler {
  (channelId: string, sampleRate: number, channels: number, pcmB64: string): void;
}

function seedThumbsFromSnapshot(snap: DashboardSnapshot) {
  for (const ch of snap.channels) {
    if (ch.thumbnail_b64) {
      liveTelemetryStore.setThumb(ch.config.id, ch.thumbnail_b64);
    }
  }
}

export function usePgmWatchSocket(onPflAudio: PflPacketHandler) {
  const [snapshot, setSnapshot] = useState<DashboardSnapshot | null>(null);
  const [connected, setConnected] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const pflHandlerRef = useRef(onPflAudio);
  pflHandlerRef.current = onPflAudio;

  useEffect(() => {
    let alive = true;
    let ws: WebSocket | null = null;
    let retryTimer: number | null = null;

    async function loadInitial() {
      try {
        const res = await fetch("/api/snapshot");
        if (res.ok && alive) {
          const data: DashboardSnapshot = await res.json();
          seedThumbsFromSnapshot(data);
          setSnapshot(data);
          setError(null);
        }
      } catch (err) {
        if (alive) {
          setError(err instanceof Error ? err.message : "Failed to connect to PGMWatch");
        }
      }
    }

    function connect() {
      const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
      ws = new WebSocket(`${proto}//${window.location.host}/ws`);
      ws.onopen = () => {
        if (!alive) return;
        setConnected(true);
        setError(null);
      };
      ws.onmessage = (ev) => {
        if (!alive) return;
        try {
          const msg = JSON.parse(ev.data);
          if (msg.event === "snapshot" && msg.snapshot) {
            const snap: DashboardSnapshot = msg.snapshot;
            const validIds = new Set(snap.channels.map((c) => c.config.id));
            liveTelemetryStore.pruneChannels(validIds);
            seedThumbsFromSnapshot(snap);
            setSnapshot(snap);
          } else if (msg.event === "thumbnail" && msg.channel_id && msg.jpeg_b64) {
            liveTelemetryStore.setThumb(msg.channel_id, msg.jpeg_b64);
          } else if (msg.event === "meter" && msg.channel_id && msg.audio) {
            liveTelemetryStore.setMeter(msg.channel_id, msg.audio);
          } else if (msg.event === "pfl_audio") {
            pflHandlerRef.current(
              msg.channel_id,
              msg.sample_rate,
              msg.channels,
              msg.pcm_s16le_b64
            );
          }
        } catch {}
      };
      ws.onclose = () => {
        if (!alive) return;
        setConnected(false);
        retryTimer = window.setTimeout(connect, 2000);
      };
      ws.onerror = () => {
        if (alive) setConnected(false);
      };
    }

    loadInitial();
    connect();

    return () => {
      alive = false;
      if (retryTimer !== null) window.clearTimeout(retryTimer);
      ws?.close();
    };
  }, []);

  const refresh = useCallback(async () => {
    const res = await fetch("/api/snapshot");
    if (res.ok) {
      const data: DashboardSnapshot = await res.json();
      seedThumbsFromSnapshot(data);
      setSnapshot(data);
    }
  }, []);

  return { snapshot, connected, error, refresh };
}
