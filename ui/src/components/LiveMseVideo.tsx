import { memo, useEffect, useRef, useState } from "react";
import {
  attachStereoRmsTap,
  getSharedAudioContext,
  StereoMeterSample,
  StereoRmsTap,
} from "../utils/audioAnalyser";

interface LiveMseVideoProps {
  channelId: string;
  streamKey: string;
  className: string;
  unmuted: boolean;
  onStatusChange?: (active: boolean, fps: number | null, hasAudio: boolean) => void;
  onAudioMeter?: (sample: StereoMeterSample | null) => void;
}

export const LiveMseVideo = memo(function LiveMseVideo({
  channelId,
  streamKey,
  className,
  unmuted,
  onStatusChange,
  onAudioMeter,
}: LiveMseVideoProps) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const statusCbRef = useRef(onStatusChange);
  statusCbRef.current = onStatusChange;
  const meterCbRef = useRef(onAudioMeter);
  meterCbRef.current = onAudioMeter;
  const unmutedRef = useRef(unmuted);
  unmutedRef.current = unmuted;
  const hasAudioRef = useRef(false);
  const [playing, setPlaying] = useState(false);
  const [tapActive, setTapActive] = useState(false);

  const audioCtxRef = useRef<AudioContext | null>(null);
  const mediaSourceRef = useRef<MediaElementAudioSourceNode | null>(null);
  const rmsTapRef = useRef<StereoRmsTap | null>(null);

  const hasActiveAudio = (v: HTMLVideoElement | null) =>
    Boolean(v && !v.muted && hasAudioRef.current);

  const ensureMediaTap = async (v: HTMLVideoElement): Promise<boolean> => {
    const ctx = getSharedAudioContext();
    if (!ctx) return false;
    audioCtxRef.current = ctx;
    if (ctx.state === "suspended") {
      try {
        await ctx.resume();
      } catch {
        return false;
      }
    }
    if (ctx.state !== "running") {
      // Never hijack <video> audio into a suspended AudioContext.
      return false;
    }
    if (!mediaSourceRef.current) {
      const src = ctx.createMediaElementSource(v);
      src.connect(ctx.destination);
      mediaSourceRef.current = src;
      rmsTapRef.current = attachStereoRmsTap(ctx, src);
    }
    return true;
  };

  useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    if (!unmuted) {
      v.muted = true;
      setTapActive(false);
      meterCbRef.current?.(null);
      if (playing) {
        statusCbRef.current?.(true, null, false);
      }
      return;
    }
    if (v.readyState < 2) return;
    let cancelled = false;
    v.muted = false;
    v.volume = 1.0;
    v.play()
      .then(async () => {
        if (cancelled || !unmutedRef.current) return;
        const active = hasActiveAudio(v);
        statusCbRef.current?.(true, null, active);
        if (active) {
          const tapped = await ensureMediaTap(v);
          if (cancelled || !unmutedRef.current) return;
          setTapActive(tapped);
          if (!tapped) {
            meterCbRef.current?.(null);
          }
        } else {
          setTapActive(false);
          meterCbRef.current?.(null);
        }
      })
      .catch(() => {
        if (cancelled) return;
        // Browser blocked unmuted playback before user gesture: keep muted 60p video playing
        // and signal parent so WebAudio PFL handles audio without stalling video.
        v.muted = true;
        setTapActive(false);
        meterCbRef.current?.(null);
        void v.play().catch(() => {});
        statusCbRef.current?.(true, null, false);
      });
    return () => {
      cancelled = true;
    };
  }, [unmuted, playing]);

  useEffect(() => {
    if (!tapActive) {
      meterCbRef.current?.(null);
      return;
    }
    let rafId = 0;
    let lastEmit = 0;
    const tick = (now: number) => {
      if (now - lastEmit >= 32) {
        lastEmit = now;
        const tap = rmsTapRef.current;
        const ctx = audioCtxRef.current;
        if (tap && ctx && ctx.state === "running") {
          meterCbRef.current?.(tap.readDb());
        }
      }
      rafId = window.requestAnimationFrame(tick);
    };
    rafId = window.requestAnimationFrame(tick);
    return () => {
      window.cancelAnimationFrame(rafId);
      meterCbRef.current?.(null);
    };
  }, [tapActive]);

  useEffect(() => {
    return () => {
      if (rmsTapRef.current) {
        rmsTapRef.current.disconnect();
        rmsTapRef.current = null;
      }
      if (mediaSourceRef.current) {
        try {
          mediaSourceRef.current.disconnect();
        } catch {}
        mediaSourceRef.current = null;
      }
      audioCtxRef.current = null;
      meterCbRef.current?.(null);
    };
  }, []);

  useEffect(() => {
    if (typeof MediaSource === "undefined") {
      statusCbRef.current?.(false, null, false);
      return;
    }

    let unmounted = false;
    let activeController: AbortController | null = null;
    let activeMediaSource: MediaSource | null = null;
    let activeObjUrl: string | null = null;
    let pruneTimer: number | null = null;
    let fpsTimer: number | null = null;
    let cleanupCanPlay: (() => void) | null = null;

    const resetSession = () => {
      cleanupCanPlay?.();
      cleanupCanPlay = null;
      if (pruneTimer !== null) {
        window.clearInterval(pruneTimer);
        pruneTimer = null;
      }
      if (fpsTimer !== null) {
        window.clearInterval(fpsTimer);
        fpsTimer = null;
      }
      activeController?.abort();
      activeController = null;
      if (videoRef.current) {
        videoRef.current.muted = true;
        videoRef.current.playbackRate = 1.0;
        videoRef.current.removeAttribute("src");
        videoRef.current.load();
      }
      if (activeMediaSource && activeMediaSource.readyState === "open") {
        try {
          activeMediaSource.endOfStream();
        } catch {}
      }
      activeMediaSource = null;
      if (activeObjUrl) {
        URL.revokeObjectURL(activeObjUrl);
        activeObjUrl = null;
      }
      hasAudioRef.current = false;
      setTapActive(false);
      meterCbRef.current?.(null);
      setPlaying(false);
      statusCbRef.current?.(false, null, false);
    };

    const waitUpdateEnd = (sb: SourceBuffer) =>
      new Promise<void>((resolve, reject) => {
        if (!sb.updating) {
          resolve();
          return;
        }
        const onEnd = () => {
          sb.removeEventListener("updateend", onEnd);
          sb.removeEventListener("error", onErr);
          resolve();
        };
        const onErr = () => {
          sb.removeEventListener("updateend", onEnd);
          sb.removeEventListener("error", onErr);
          reject(new Error("SourceBuffer error"));
        };
        sb.addEventListener("updateend", onEnd);
        sb.addEventListener("error", onErr);
      });

    async function runLoop() {
      while (!unmounted) {
        const controller = new AbortController();
        activeController = controller;
        let playbackStarted = false;
        const sessionStartAt = performance.now();

        const tryStartPlayback = () => {
          const v = videoRef.current;
          if (!v || unmounted || v.buffered.length === 0) return;
          const start = v.buffered.start(0);
          const end = v.buffered.end(v.buffered.length - 1);
          const span = end - start;
          const elapsed = performance.now() - sessionStartAt;

          if (!playbackStarted) {
            // Buffer a small lookahead before playing so fragment boundaries never underrun/stutter,
            // then seek straight to the live edge of the buffered window instead of replaying old GOP frames.
            if (span < 0.7 && !(elapsed > 1400 && span >= 0.35)) {
              return;
            }
            const target = Math.max(start, end - Math.min(0.55, span * 0.35));
            if (v.currentTime < start || Math.abs(v.currentTime - target) > 0.2) {
              v.currentTime = target;
            }
            playbackStarted = true;
          }

          v.muted = !unmutedRef.current;
          if (v.paused && v.readyState >= 2) {
            v.play().catch(() => {
              if (!v.muted) {
                v.muted = true;
                void v.play().catch(() => {});
                statusCbRef.current?.(true, null, false);
              }
            });
          }
        };

        const syncLiveEdge = (sb: SourceBuffer | null) => {
          const v = videoRef.current;
          if (!v || v.buffered.length === 0) return;
          if (!playbackStarted) {
            tryStartPlayback();
            return;
          }
          const start = v.buffered.start(0);
          const end = v.buffered.end(v.buffered.length - 1);

          if (v.currentTime < start - 0.05) {
            v.currentTime = Math.max(start, end - 0.7);
            if (v.playbackRate !== 1.0) v.playbackRate = 1.0;
          } else {
            const lag = end - v.currentTime;
            if (lag > 7.0) {
              v.currentTime = Math.max(start, end - 0.8);
              if (v.playbackRate !== 1.0) v.playbackRate = 1.0;
            } else if (lag > 3.2) {
              if (v.playbackRate !== 1.06) v.playbackRate = 1.06;
            } else if (lag <= 1.6 && v.playbackRate !== 1.0) {
              v.playbackRate = 1.0;
            }
          }

          if (sb && !sb.updating && v.currentTime > start + 16) {
            try {
              sb.remove(start, v.currentTime - 10);
            } catch {}
          }
        };

        try {
          const res = await fetch(
            `/api/channels/${encodeURIComponent(channelId)}/stream.fmp4`,
            {
              signal: controller.signal,
              cache: "no-store",
            }
          );

          if (!res.ok || !res.body) {
            const retryMs = res.status === 503 || res.status === 502 ? 1000 : 2500;
            resetSession();
            if (!unmounted) {
              await new Promise((r) => setTimeout(r, retryMs));
            }
            continue;
          }

          const codec = res.headers.get("X-MSE-Codec") ?? "avc1.64002a";
          hasAudioRef.current = codec.includes("mp4a");
          const mime = `video/mp4; codecs="${codec}"`;
          if (!MediaSource.isTypeSupported(mime)) {
            console.warn(`PGMWatch MSE unsupported codec for ${channelId}: ${mime}`);
            resetSession();
            return;
          }

          const video = videoRef.current;
          if (!video || unmounted) return;

          const mediaSource = new MediaSource();
          activeMediaSource = mediaSource;
          const objUrl = URL.createObjectURL(mediaSource);
          activeObjUrl = objUrl;
          video.src = objUrl;

          const onCanPlay = () => tryStartPlayback();
          video.addEventListener("canplay", onCanPlay);
          video.addEventListener("loadeddata", onCanPlay);
          cleanupCanPlay = () => {
            video.removeEventListener("canplay", onCanPlay);
            video.removeEventListener("loadeddata", onCanPlay);
          };

          await new Promise<void>((resolve, reject) => {
            const onOpen = () => {
              mediaSource.removeEventListener("sourceopen", onOpen);
              resolve();
            };
            mediaSource.addEventListener("sourceopen", onOpen);
            controller.signal.addEventListener(
              "abort",
              () => reject(new DOMException("Aborted", "AbortError")),
              { once: true }
            );
          });

          if (unmounted) break;
          const sb = mediaSource.addSourceBuffer(mime);

          const appendChunk = async (chunk: Uint8Array) => {
            while (sb.updating) {
              await waitUpdateEnd(sb);
            }
            if (unmounted || mediaSource.readyState !== "open") return;
            const copy = new Uint8Array(chunk.byteLength);
            copy.set(chunk);
            sb.appendBuffer(copy);
            await waitUpdateEnd(sb);
          };

          pruneTimer = window.setInterval(() => {
            syncLiveEdge(sb);
          }, 1000);

          let prevFrames = 0;
          let prevAt = performance.now();
          let lastReportedFps = -1;
          fpsTimer = window.setInterval(() => {
            const v = videoRef.current;
            if (!v || v.readyState < 2) return;
            const total =
              typeof v.getVideoPlaybackQuality === "function"
                ? v.getVideoPlaybackQuality().totalVideoFrames
                : 0;
            const now = performance.now();
            const dt = (now - prevAt) / 1000;
            if (dt > 0.3 && total >= prevFrames) {
              const measured = Math.round((total - prevFrames) / dt);
              if (measured > 0 && measured !== lastReportedFps) {
                lastReportedFps = measured;
                statusCbRef.current?.(true, measured, hasActiveAudio(v));
              }
            }
            prevFrames = total;
            prevAt = now;
          }, 1000);

          const reader = res.body.getReader();
          while (!unmounted) {
            const { done, value } = await reader.read();
            if (done || !value) break;
            await appendChunk(value);
            tryStartPlayback();
          }
        } catch (err) {
          if (!unmounted && !(err instanceof DOMException && err.name === "AbortError")) {
            console.warn(`PGMWatch MSE stream reconnecting for ${channelId}:`, err);
          }
        }

        resetSession();
        if (!unmounted) {
          await new Promise((r) => setTimeout(r, 1200));
        }
      }
    }

    void runLoop();
    return () => {
      unmounted = true;
      resetSession();
    };
  }, [channelId, streamKey]);

  return (
    <video
      ref={videoRef}
      muted={!unmuted}
      autoPlay
      playsInline
      onPlaying={() => {
        setPlaying(true);
        statusCbRef.current?.(true, null, hasActiveAudio(videoRef.current));
      }}
      className={`${className} ${playing ? "opacity-100" : "opacity-0"} transition-opacity duration-200`}
    />
  );
});
