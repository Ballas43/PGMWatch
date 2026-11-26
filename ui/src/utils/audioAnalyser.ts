export interface StereoMeterSample {
  leftDb: number;
  rightDb: number;
}

export interface StereoRmsTap {
  readDb: () => StereoMeterSample;
  disconnect: () => void;
}

let sharedAudioContext: AudioContext | null = null;

/**
 * Returns a single app-wide AudioContext shared across WebAudio PFL and MSE media taps,
 * avoiding per-card AudioContext leaks and browser hardware context limits.
 */
export function getSharedAudioContext(): AudioContext | null {
  if (typeof window === "undefined") return null;
  const AudioCtx = window.AudioContext;
  if (!AudioCtx) return null;
  if (!sharedAudioContext || sharedAudioContext.state === "closed") {
    sharedAudioContext = new AudioCtx();
  }
  return sharedAudioContext;
}

/**
 * Synchronously creates and resumes the shared AudioContext inside a user gesture handler
 * so subsequent async playback / MediaElementSource taps can attach in the running state.
 */
export function unlockSharedAudioContext(): AudioContext | null {
  const ctx = getSharedAudioContext();
  if (ctx && ctx.state === "suspended") {
    void ctx.resume().catch(() => {});
  }
  return ctx;
}

export function computeRmsDbfs(buf: Float32Array): number {
  let sumSq = 0;
  for (let i = 0; i < buf.length; i++) {
    const v = buf[i];
    sumSq += v * v;
  }
  const rms = Math.sqrt(sumSq / Math.max(1, buf.length));
  if (rms <= 1e-6) return -100;
  return Math.max(-100, Math.min(0, 20 * Math.log10(rms)));
}

/**
 * Taps a Web Audio node through a stereo splitter and dual AnalyserNodes so VU bars
 * reflect the exact speaker-clocked playout waveform at 0ms A/V offset.
 */
export function attachStereoRmsTap(ctx: AudioContext, source: AudioNode): StereoRmsTap {
  const tapBus = ctx.createGain();
  tapBus.channelCount = 2;
  tapBus.channelCountMode = "explicit";
  tapBus.channelInterpretation = "speakers";

  const splitter = ctx.createChannelSplitter(2);
  const analyserL = ctx.createAnalyser();
  const analyserR = ctx.createAnalyser();
  analyserL.fftSize = 1024;
  analyserR.fftSize = 1024;
  analyserL.smoothingTimeConstant = 0.25;
  analyserR.smoothingTimeConstant = 0.25;

  source.connect(tapBus);
  tapBus.connect(splitter);
  splitter.connect(analyserL, 0);
  splitter.connect(analyserR, 1);

  const bufL = new Float32Array(analyserL.fftSize);
  const bufR = new Float32Array(analyserR.fftSize);

  return {
    readDb: () => {
      analyserL.getFloatTimeDomainData(bufL);
      analyserR.getFloatTimeDomainData(bufR);
      return {
        leftDb: computeRmsDbfs(bufL),
        rightDb: computeRmsDbfs(bufR),
      };
    },
    disconnect: () => {
      try {
        source.disconnect(tapBus);
      } catch {
        // Ignore if source already torn down
      }
      try {
        tapBus.disconnect();
      } catch {
        // Ignore
      }
      try {
        splitter.disconnect();
      } catch {
        // Ignore
      }
    },
  };
}
