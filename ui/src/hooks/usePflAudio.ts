import { useCallback, useEffect, useRef, useState } from "react";
import {
  attachStereoRmsTap,
  getSharedAudioContext,
  StereoMeterSample,
  StereoRmsTap,
  unlockSharedAudioContext,
} from "../utils/audioAnalyser";

export function usePflAudio() {
  const ctxRef = useRef<AudioContext | null>(null);
  const gainRef = useRef<GainNode | null>(null);
  const tapRef = useRef<StereoRmsTap | null>(null);
  const activeSourcesRef = useRef<Set<AudioBufferSourceNode>>(new Set());
  const nextTimeRef = useRef<number>(0);
  const allowedChannelRef = useRef<string | null>(null);
  const [allowedChannel, setAllowedChannelState] = useState<string | null>(null);
  const [muted, setMutedState] = useState(false);
  const [pflMeter, setPflMeter] = useState<StereoMeterSample | null>(null);
  const mutedRef = useRef(false);
  mutedRef.current = muted;

  const ensureGainAndTap = useCallback((ctx: AudioContext) => {
    if (!gainRef.current) {
      const gain = ctx.createGain();
      gain.gain.value = mutedRef.current ? 0 : 1;
      gain.connect(ctx.destination);
      gainRef.current = gain;
    }
    if (!tapRef.current && gainRef.current) {
      tapRef.current = attachStereoRmsTap(ctx, gainRef.current);
    }
  }, []);

  const stopAll = useCallback(() => {
    for (const src of activeSourcesRef.current) {
      try {
        src.stop();
      } catch {}
      try {
        src.disconnect();
      } catch {}
    }
    activeSourcesRef.current.clear();

    if (tapRef.current) {
      tapRef.current.disconnect();
      tapRef.current = null;
    }

    if (gainRef.current) {
      try {
        gainRef.current.disconnect();
      } catch {}
      gainRef.current = null;
    }

    if (ctxRef.current) {
      nextTimeRef.current = ctxRef.current.currentTime;
    } else {
      nextTimeRef.current = 0;
    }
    setPflMeter(null);
  }, []);

  const ensureContext = useCallback(() => {
    const ctx = unlockSharedAudioContext();
    if (!ctx) return null;
    if (ctxRef.current !== ctx) {
      ctxRef.current = ctx;
      nextTimeRef.current = ctx.currentTime;
    }
    ensureGainAndTap(ctx);
    return ctx;
  }, [ensureGainAndTap]);

  const setAllowedChannel = useCallback(
    (channelId: string | null) => {
      if (allowedChannelRef.current !== channelId) {
        allowedChannelRef.current = channelId;
        setAllowedChannelState(channelId);
        stopAll();
      }
    },
    [stopAll]
  );

  const setMuted = useCallback(
    (next: boolean | ((prev: boolean) => boolean)) => {
      const val = typeof next === "function" ? next(mutedRef.current) : next;
      mutedRef.current = val;
      setMutedState(val);
      if (val) {
        stopAll();
      } else {
        unlockSharedAudioContext();
        if (gainRef.current) {
          gainRef.current.gain.value = 1;
        }
      }
    },
    [stopAll]
  );

  useEffect(() => {
    if (!allowedChannel || muted) {
      setPflMeter(null);
      return;
    }

    let rafId = 0;
    let lastEmit = 0;
    const tick = (now: number) => {
      if (now - lastEmit >= 32) {
        lastEmit = now;
        const tap = tapRef.current;
        const ctx = ctxRef.current;
        if (tap && ctx && ctx.state === "running" && activeSourcesRef.current.size > 0) {
          setPflMeter(tap.readDb());
        } else if (ctx && ctx.state === "running" && allowedChannelRef.current === allowedChannel) {
          setPflMeter((prev) =>
            prev && prev.leftDb === -100 && prev.rightDb === -100
              ? prev
              : { leftDb: -100, rightDb: -100 }
          );
        }
      }
      rafId = window.requestAnimationFrame(tick);
    };
    rafId = window.requestAnimationFrame(tick);

    return () => {
      window.cancelAnimationFrame(rafId);
      setPflMeter(null);
    };
  }, [allowedChannel, muted]);

  useEffect(() => {
    return () => {
      stopAll();
      ctxRef.current = null;
    };
  }, [stopAll]);

  const pushPcmPacket = useCallback(
    (channelId: string, sampleRate: number, channels: number, pcmB64: string) => {
      // Strictly ignore packets if PFL is muted, off, or belonging to another channel
      if (mutedRef.current || allowedChannelRef.current !== channelId) return;
      let ctx = ctxRef.current;
      if (!ctx) {
        ctx = getSharedAudioContext();
        if (!ctx) return;
        ctxRef.current = ctx;
        nextTimeRef.current = ctx.currentTime;
      }
      if (ctx.state === "suspended") {
        void ctx.resume();
        return;
      }
      if (ctx.state !== "running") return;

      ensureGainAndTap(ctx);
      if (!gainRef.current) return;

      const bin = atob(pcmB64);
      const bytes = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) {
        bytes[i] = bin.charCodeAt(i);
      }
      const view = new DataView(bytes.buffer);
      const chCount = Math.max(1, channels);
      const frames = Math.floor(bytes.length / (2 * chCount));
      if (frames === 0) return;

      const now = ctx.currentTime;
      // Drop excess packet on severe overrun instead of overlapping on top of active buffers.
      if (nextTimeRef.current > now + 0.85) {
        return;
      }

      const underrun = nextTimeRef.current < now + 0.015;
      if (underrun) {
        // 180ms jitter buffer absorbs 100ms WS chunk cadence and HLS segment reload bursts.
        nextTimeRef.current = now + 0.18;
      }

      const audioBuf = ctx.createBuffer(chCount, frames, sampleRate);
      const fadeSamples = underrun ? Math.min(frames, Math.floor(sampleRate * 0.003)) : 0;
      for (let c = 0; c < chCount; c++) {
        const channelData = audioBuf.getChannelData(c);
        for (let f = 0; f < frames; f++) {
          let sample = view.getInt16((f * chCount + c) * 2, true) / 32768;
          if (f < fadeSamples) {
            sample *= f / fadeSamples;
          }
          channelData[f] = sample;
        }
      }

      const src = ctx.createBufferSource();
      src.buffer = audioBuf;
      src.connect(gainRef.current);
      activeSourcesRef.current.add(src);
      src.onended = () => {
        activeSourcesRef.current.delete(src);
        try {
          src.disconnect();
        } catch {}
      };
      const startAt = nextTimeRef.current;
      src.start(startAt);
      nextTimeRef.current = startAt + audioBuf.duration;
    },
    [ensureGainAndTap]
  );

  return { ensureContext, setAllowedChannel, stopAll, pushPcmPacket, muted, setMuted, pflMeter };
}
