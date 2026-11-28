import { useSyncExternalStore } from "react";
import { AudioTelemetry } from "../types";
import { StereoMeterSample } from "./audioAnalyser";

type Listener = () => void;

class LiveTelemetryStore {
  private thumbs = new Map<string, string>();
  private thumbListeners = new Map<string, Set<Listener>>();

  private meters = new Map<string, AudioTelemetry>();
  private meterListeners = new Map<string, Set<Listener>>();

  private pflMeter: StereoMeterSample | null = null;
  private pflListeners = new Set<Listener>();

  setThumb(channelId: string, jpegB64: string) {
    if (this.thumbs.get(channelId) === jpegB64) return;
    this.thumbs.set(channelId, jpegB64);
    const set = this.thumbListeners.get(channelId);
    if (set) {
      for (const cb of set) cb();
    }
  }

  getThumb(channelId: string): string | undefined {
    return this.thumbs.get(channelId);
  }

  subscribeThumb(channelId: string, cb: Listener): () => void {
    let set = this.thumbListeners.get(channelId);
    if (!set) {
      set = new Set();
      this.thumbListeners.set(channelId, set);
    }
    set.add(cb);
    return () => {
      const cur = this.thumbListeners.get(channelId);
      if (!cur) return;
      cur.delete(cb);
      if (cur.size === 0) this.thumbListeners.delete(channelId);
    };
  }

  setMeter(channelId: string, audio: AudioTelemetry) {
    this.meters.set(channelId, audio);
    const set = this.meterListeners.get(channelId);
    if (set) {
      for (const cb of set) cb();
    }
  }

  getMeter(channelId: string): AudioTelemetry | undefined {
    return this.meters.get(channelId);
  }

  subscribeMeter(channelId: string, cb: Listener): () => void {
    let set = this.meterListeners.get(channelId);
    if (!set) {
      set = new Set();
      this.meterListeners.set(channelId, set);
    }
    set.add(cb);
    return () => {
      const cur = this.meterListeners.get(channelId);
      if (!cur) return;
      cur.delete(cb);
      if (cur.size === 0) this.meterListeners.delete(channelId);
    };
  }

  pruneChannels(validIds: Set<string>) {
    for (const id of Array.from(this.thumbs.keys())) {
      if (!validIds.has(id)) {
        this.thumbs.delete(id);
      }
    }
    for (const id of Array.from(this.meters.keys())) {
      if (!validIds.has(id)) {
        this.meters.delete(id);
      }
    }
  }

  setPflMeter(sample: StereoMeterSample | null) {
    const prev = this.pflMeter;
    if (prev === null && sample === null) return;
    if (prev && sample && prev.leftDb === sample.leftDb && prev.rightDb === sample.rightDb) {
      return;
    }
    this.pflMeter = sample;
    for (const cb of this.pflListeners) cb();
  }

  getPflMeter(): StereoMeterSample | null {
    return this.pflMeter;
  }

  subscribePflMeter(cb: Listener): () => void {
    this.pflListeners.add(cb);
    return () => {
      this.pflListeners.delete(cb);
    };
  }
}

export const liveTelemetryStore = new LiveTelemetryStore();

const noopUnsub = () => {};
const getNull = () => null;

export function useChannelThumb(channelId: string): string | undefined {
  return useSyncExternalStore(
    (cb) => liveTelemetryStore.subscribeThumb(channelId, cb),
    () => liveTelemetryStore.getThumb(channelId)
  );
}

export function useChannelMeter(channelId: string): AudioTelemetry | undefined {
  return useSyncExternalStore(
    (cb) => liveTelemetryStore.subscribeMeter(channelId, cb),
    () => liveTelemetryStore.getMeter(channelId)
  );
}

export function usePflMeter(active: boolean): StereoMeterSample | null {
  return useSyncExternalStore(
    (cb) => (active ? liveTelemetryStore.subscribePflMeter(cb) : noopUnsub),
    () => (active ? liveTelemetryStore.getPflMeter() : getNull())
  );
}
