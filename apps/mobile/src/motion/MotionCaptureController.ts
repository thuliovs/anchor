import type { EventSubscription } from 'react-native';
import type { MotionSampleV1 } from '@anchor/protocol';

import { convertNativeMotionFrameToSample } from './motionSample';
import {
  getMotionSensorsModule,
  type MotionSensorsModule,
  type SensorAvailability,
} from './nativeMotionSensors';

export type MotionCaptureStatus =
  | 'checking_sensors'
  | 'ready'
  | 'starting'
  | 'active'
  | 'stale'
  | 'stopped'
  | 'unsupported'
  | 'error';

export interface MotionCaptureSnapshot {
  status: MotionCaptureStatus;
  availability: SensorAvailability | null;
  sessionId: string | null;
  sequence: number | null;
  sessionElapsedUs: number | null;
  acceptedCount: number;
  rejectedCount: number;
  observedRateHz: number;
  lastSampleAgeMs: number | null;
  lastSample: MotionSampleV1 | null;
  errorMessage: string | null;
  wasInterrupted: boolean;
}

interface AppStateSubscription {
  remove(): void;
}

interface AppStateLike {
  addEventListener(
    eventType: 'change',
    listener: (state: string) => void,
  ): AppStateSubscription;
}

interface TimerLike {
  setInterval(handler: () => void, intervalMs: number): ReturnType<typeof setInterval>;
  clearInterval(handle: ReturnType<typeof setInterval>): void;
}

interface MotionCaptureControllerOptions {
  nativeModule?: MotionSensorsModule | null;
  now?: () => number;
  appState?: AppStateLike;
  timers?: TimerLike;
  captureRateHz?: number;
  staleThresholdMs?: number;
  uiRefreshIntervalMs?: number;
}

class ObservedRateTracker {
  private readonly timestampsMs: number[] = [];

  record(timestampMs: number): number {
    this.timestampsMs.push(timestampMs);
    this.trim(timestampMs);

    if (this.timestampsMs.length < 2) {
      return 0;
    }

    const first = this.timestampsMs[0];
    const last = this.timestampsMs[this.timestampsMs.length - 1];
    if (last <= first) {
      return 0;
    }

    return ((this.timestampsMs.length - 1) * 1000) / (last - first);
  }

  reset(): void {
    this.timestampsMs.length = 0;
  }

  private trim(nowMs: number): void {
    const threshold = nowMs - 1000;
    while (this.timestampsMs.length > 1 && this.timestampsMs[0] < threshold) {
      this.timestampsMs.shift();
    }
  }
}

export class MotionCaptureController {
  private readonly listeners = new Set<(snapshot: MotionCaptureSnapshot) => void>();
  private readonly nativeModule: MotionSensorsModule | null;
  private readonly now: () => number;
  private readonly appState: AppStateLike;
  private readonly timers: TimerLike;
  private readonly captureRateHz: number;
  private readonly staleThresholdMs: number;
  private readonly uiRefreshIntervalMs: number;
  private readonly rateTracker = new ObservedRateTracker();

  private snapshot: MotionCaptureSnapshot = {
    status: 'checking_sensors',
    availability: null,
    sessionId: null,
    sequence: null,
    sessionElapsedUs: null,
    acceptedCount: 0,
    rejectedCount: 0,
    observedRateHz: 0,
    lastSampleAgeMs: null,
    lastSample: null,
    errorMessage: null,
    wasInterrupted: false,
  };

  private initialized = false;
  private disposed = false;
  private captureActive = false;
  private appStateSubscription: AppStateSubscription | null = null;
  private frameSubscription: EventSubscription | null = null;
  private uiInterval: ReturnType<typeof setInterval> | null = null;
  private lastFrameReceivedAtMs: number | null = null;
  private captureStartRequestedAtMs: number | null = null;
  private captureGeneration = 0;
  private activeCaptureGeneration: number | null = null;
  private pendingStartGeneration: number | null = null;

  constructor(options: MotionCaptureControllerOptions = {}) {
    this.nativeModule = options.nativeModule ?? getMotionSensorsModule();
    this.now = options.now ?? defaultNow;
    this.appState = options.appState ?? defaultAppState;
    this.timers = options.timers ?? defaultTimers;
    this.captureRateHz = options.captureRateHz ?? 60;
    this.staleThresholdMs = options.staleThresholdMs ?? 250;
    this.uiRefreshIntervalMs = options.uiRefreshIntervalMs ?? 100;
  }

  subscribe(listener: (snapshot: MotionCaptureSnapshot) => void): () => void {
    this.listeners.add(listener);
    listener(this.snapshot);

    return () => {
      this.listeners.delete(listener);
    };
  }

  getSnapshot(): MotionCaptureSnapshot {
    return this.snapshot;
  }

  async initialize(): Promise<void> {
    if (this.initialized || this.disposed) {
      return;
    }

    this.initialized = true;
    this.appStateSubscription = this.appState.addEventListener('change', state => {
      if (state === 'inactive' || state === 'background') {
        this.stopCapture('lifecycle');
      }
    });
    this.uiInterval = this.timers.setInterval(() => {
      this.refreshTemporalState();
    }, this.uiRefreshIntervalMs);

    if (!this.nativeModule) {
      this.updateSnapshot({
        status: 'unsupported',
        errorMessage: 'Turbo Native Module indisponivel neste ambiente.',
      });
      return;
    }

    try {
      const availability = await this.nativeModule.getAvailability();
      if (this.disposed) {
        return;
      }

      const supported = availability.linearAcceleration
        && availability.gravity
        && availability.gyroscope;

      this.updateSnapshot({
        availability,
        status: supported ? 'ready' : 'unsupported',
        errorMessage: supported ? null : 'Sensores obrigatorios indisponiveis neste dispositivo.',
      });
    } catch (error) {
      if (this.disposed) {
        return;
      }

      this.updateSnapshot({
        status: 'error',
        errorMessage: toMessage(error),
      });
    }
  }

  async startCapture(): Promise<void> {
    if (this.disposed || !this.nativeModule) {
      this.updateSnapshot({
        status: 'unsupported',
        errorMessage: 'Turbo Native Module indisponivel neste ambiente.',
      });
      return;
    }

    if (this.snapshot.status === 'checking_sensors') {
      return;
    }

    if (this.captureActive || this.snapshot.status === 'starting') {
      return;
    }

    if (!this.snapshot.availability?.linearAcceleration
      || !this.snapshot.availability.gravity
      || !this.snapshot.availability.gyroscope) {
      return;
    }

    const generation = this.nextCaptureGeneration();
    this.captureActive = true;
    this.activeCaptureGeneration = generation;
    this.pendingStartGeneration = generation;
    this.lastFrameReceivedAtMs = null;
    this.captureStartRequestedAtMs = this.now();
    this.rateTracker.reset();
    this.frameSubscription = this.nativeModule.onMotionFrame(frame => {
      this.handleNativeFrame(frame, generation);
    });

    this.updateSnapshot({
      status: 'starting',
      sessionId: null,
      sequence: this.snapshot.lastSample?.sequence ?? null,
      sessionElapsedUs: this.snapshot.lastSample?.sessionElapsedUs ?? null,
      acceptedCount: 0,
      rejectedCount: 0,
      observedRateHz: 0,
      lastSampleAgeMs: null,
      errorMessage: null,
      wasInterrupted: false,
    });

    try {
      const result = await this.nativeModule.start(this.captureRateHz);
      if (!this.isCurrentCaptureGeneration(generation)) {
        return;
      }

      this.pendingStartGeneration = null;
      this.updateSnapshot({ sessionId: result.sessionId });
    } catch (error) {
      if (!this.isCurrentCaptureGeneration(generation)) {
        return;
      }

      this.pendingStartGeneration = null;
      this.teardownCaptureRuntime();
      this.activeCaptureGeneration = null;
      this.updateSnapshot({
        status: 'error',
        errorMessage: toMessage(error),
      });
    }
  }

  stopCapture(reason: 'explicit' | 'lifecycle' = 'explicit'): void {
    if (!this.captureActive
      && this.activeCaptureGeneration === null
      && this.pendingStartGeneration === null
      && !this.frameSubscription) {
      return;
    }

    this.invalidateCaptureGeneration();

    if (this.nativeModule) {
      this.nativeModule.stop();
    }

    this.teardownCaptureRuntime();

    this.updateSnapshot({
      status: 'stopped',
      observedRateHz: 0,
      lastSampleAgeMs: this.snapshot.lastSampleAgeMs,
      wasInterrupted: reason === 'lifecycle',
    });
  }

  dispose(): void {
    if (this.disposed) {
      return;
    }

    this.disposed = true;
    this.stopCapture('explicit');
    this.appStateSubscription?.remove();
    this.appStateSubscription = null;
    if (this.uiInterval) {
      this.timers.clearInterval(this.uiInterval);
      this.uiInterval = null;
    }
    this.listeners.clear();
  }

  private handleNativeFrame(
    frame: Parameters<MotionSensorsModule['onMotionFrame']>[0] extends (arg: infer T) => void ? T : never,
    generation: number | null,
  ): void {
    if (this.disposed || !this.captureActive || generation === null || !this.isCurrentCaptureGeneration(generation)) {
      return;
    }

    const converted = convertNativeMotionFrameToSample(frame);
    if (!converted.ok) {
      this.snapshot = {
        ...this.snapshot,
        rejectedCount: this.snapshot.rejectedCount + 1,
      };
      return;
    }

    const nowMs = this.now();
    const observedRateHz = this.rateTracker.record(nowMs);
    this.lastFrameReceivedAtMs = nowMs;
    this.pendingStartGeneration = null;
    this.snapshot = {
      ...this.snapshot,
      status: 'active',
      acceptedCount: this.snapshot.acceptedCount + 1,
      observedRateHz,
      lastSampleAgeMs: 0,
      lastSample: converted.sample,
      sessionId: converted.sample.sessionId,
      sequence: converted.sample.sequence,
      sessionElapsedUs: converted.sample.sessionElapsedUs,
      errorMessage: null,
    };
  }

  private refreshTemporalState(): void {
    if (this.disposed) {
      return;
    }

    if (!this.captureActive || this.activeCaptureGeneration === null) {
      return;
    }

    if (!this.snapshot.lastSample || this.lastFrameReceivedAtMs === null) {
      if (this.captureStartRequestedAtMs === null) {
        return;
      }

      const waitMs = Math.max(0, Math.round(this.now() - this.captureStartRequestedAtMs));
      if (waitMs > this.staleThresholdMs && this.snapshot.status !== 'stale') {
        this.updateSnapshot({ status: 'stale' });
      }
      return;
    }

    const ageMs = Math.max(0, Math.round(this.now() - this.lastFrameReceivedAtMs));
    const status = ageMs > this.staleThresholdMs ? 'stale' : 'active';
    this.updateSnapshot({ lastSampleAgeMs: ageMs, status });
  }

  private teardownCaptureRuntime(): void {
    this.captureActive = false;
    this.frameSubscription?.remove();
    this.frameSubscription = null;
    this.lastFrameReceivedAtMs = null;
    this.captureStartRequestedAtMs = null;
    this.rateTracker.reset();
  }

  private nextCaptureGeneration(): number {
    this.captureGeneration += 1;
    return this.captureGeneration;
  }

  private invalidateCaptureGeneration(): void {
    this.captureGeneration += 1;
    this.activeCaptureGeneration = null;
    this.pendingStartGeneration = null;
  }

  private isCurrentCaptureGeneration(generation: number): boolean {
    return !this.disposed && this.activeCaptureGeneration === generation;
  }

  private updateSnapshot(patch: Partial<MotionCaptureSnapshot>): void {
    this.snapshot = {
      ...this.snapshot,
      ...patch,
    };
    this.notify();
  }

  private notify(): void {
    this.listeners.forEach(listener => listener(this.snapshot));
  }
}

const defaultTimers: TimerLike = {
  setInterval: (handler, intervalMs) => setInterval(handler, intervalMs),
  clearInterval: handle => clearInterval(handle),
};

const defaultAppState: AppStateLike = {
  addEventListener: (eventType, listener) => {
    const { AppState } = require('react-native') as typeof import('react-native');
    return AppState.addEventListener(eventType, listener);
  },
};

function defaultNow(): number {
  const perf = (globalThis as { performance?: { now?: () => number } }).performance;
  if (perf && typeof perf.now === 'function') {
    return perf.now();
  }

  return Date.now();
}

function toMessage(error: unknown): string {
  if (error instanceof Error && error.message) {
    return error.message;
  }

  return 'Falha inesperada ao acessar sensores.';
}
