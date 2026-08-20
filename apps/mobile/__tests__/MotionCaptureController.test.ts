import type { EventSubscription } from 'react-native';

import { MotionCaptureController } from '../src/motion/MotionCaptureController';
import type {
  MotionSensorsModule,
  NativeMotionFrame,
  SensorAvailability,
} from '../src/motion/nativeMotionSensors';

class FakeAppState {
  private listener: ((state: string) => void) | null = null;

  addEventListener(_eventType: 'change', listener: (state: string) => void) {
    this.listener = listener;
    return {
      remove: () => {
        if (this.listener === listener) {
          this.listener = null;
        }
      },
    };
  }

  emit(state: string) {
    this.listener?.(state);
  }
}

class FakeMotionModule implements MotionSensorsModule {
  public readonly subscriptions: FakeSubscription[] = [];
  public readonly startCalls: number[] = [];
  public stopCallCount = 0;
  public availability: SensorAvailability = {
    linearAcceleration: true,
    gravity: true,
    gyroscope: true,
  };
  public startResult = {
    sessionId: 'native-session',
    requestedRateHz: 60,
  };
  public pendingStart: Deferred<{ sessionId: string; requestedRateHz: number }> | null = null;
  public startMode: 'immediate' | 'deferred' = 'immediate';

  async getAvailability(): Promise<SensorAvailability> {
    return this.availability;
  }

  async start(rateHz: number) {
    this.startCalls.push(rateHz);

    if (this.startMode === 'deferred') {
      this.pendingStart = createDeferred();
      return this.pendingStart.promise;
    }

    return this.startResult;
  }

  stop(): void {
    this.stopCallCount += 1;
  }

  onMotionFrame(listener: (frame: NativeMotionFrame) => void): EventSubscription {
    const subscription = new FakeSubscription(listener, () => {
      const index = this.subscriptions.indexOf(subscription);
      if (index >= 0) {
        this.subscriptions.splice(index, 1);
      }
    });
    this.subscriptions.push(subscription);
    return subscription as unknown as EventSubscription;
  }

  emit(frame: NativeMotionFrame) {
    this.subscriptions.forEach(subscription => subscription.emit(frame));
  }
}

class FakeSubscription {
  public removed = false;

  constructor(
    private readonly listener: (frame: NativeMotionFrame) => void,
    private readonly onRemove: () => void,
  ) {}

  remove(): void {
    this.removed = true;
    this.onRemove();
  }

  emit(frame: NativeMotionFrame) {
    if (!this.removed) {
      this.listener(frame);
    }
  }
}

function buildFrame(overrides: Partial<NativeMotionFrame> = {}): NativeMotionFrame {
  return {
    sessionId: 'native-session',
    sequence: 0,
    sessionElapsedUs: 1_000,
    linearAccelerationMps2: { x: 0, y: 0, z: 0 },
    gravityMps2: { x: 0, y: 0, z: -9.80665 },
    angularVelocityRadS: { x: 0, y: 0, z: 0 },
    ...overrides,
  };
}

type Deferred<T> = {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason?: unknown) => void;
};

function createDeferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });

  return { promise, resolve, reject };
}

describe('MotionCaptureController', () => {
  beforeEach(() => {
    jest.useFakeTimers();
  });

  afterEach(() => {
    jest.useRealTimers();
  });

  it('exige inicio explicito e suporta stop idempotente', async () => {
    const appState = new FakeAppState();
    const module = new FakeMotionModule();
    let nowMs = 0;
    const controller = new MotionCaptureController({
      nativeModule: module,
      now: () => nowMs,
      appState,
    });

    await controller.initialize();

    expect(controller.getSnapshot().status).toBe('ready');
    expect(module.startCalls).toHaveLength(0);

    await controller.startCapture();
    expect(module.startCalls).toEqual([60]);

    controller.stopCapture();
    controller.stopCapture();
    expect(module.stopCallCount).toBe(1);
    expect(controller.getSnapshot().status).toBe('stopped');
  });

  it('preserva a ultima amostra, calcula taxa observada e transita de active para stale', async () => {
    const appState = new FakeAppState();
    const module = new FakeMotionModule();
    let nowMs = 0;
    const controller = new MotionCaptureController({
      nativeModule: module,
      now: () => nowMs,
      appState,
    });

    await controller.initialize();
    await controller.startCapture();

    module.emit(buildFrame({ sequence: 0, sessionElapsedUs: 1_000 }));
    nowMs = 100;
    module.emit(buildFrame({ sequence: 1, sessionElapsedUs: 17_000 }));
    nowMs = 200;
    module.emit(buildFrame({ sequence: 2, sessionElapsedUs: 33_000 }));

    jest.advanceTimersByTime(100);

    let snapshot = controller.getSnapshot();
    expect(snapshot.status).toBe('active');
    expect(snapshot.acceptedCount).toBe(3);
    expect(snapshot.lastSample?.sequence).toBe(2);
    expect(snapshot.observedRateHz).toBeGreaterThan(0);

    nowMs = 520;
    jest.advanceTimersByTime(300);

    snapshot = controller.getSnapshot();
    expect(snapshot.status).toBe('stale');
    expect(snapshot.lastSample?.sequence).toBe(2);
    expect(snapshot.lastSampleAgeMs).toBe(320);
  });

  it('rejeita frames invalidos e contabiliza rejeicoes', async () => {
    const module = new FakeMotionModule();
    const controller = new MotionCaptureController({
      nativeModule: module,
      now: () => 0,
      appState: new FakeAppState(),
    });

    await controller.initialize();
    await controller.startCapture();

    module.emit(buildFrame({ gravityMps2: { x: 0, y: 0, z: -25 } }));
    jest.advanceTimersByTime(100);

    const snapshot = controller.getSnapshot();
    expect(snapshot.acceptedCount).toBe(0);
    expect(snapshot.rejectedCount).toBe(1);
  });

  it('cleanup remove subscricao e timers', async () => {
    const appState = new FakeAppState();
    const module = new FakeMotionModule();
    const clearIntervalSpy = jest.fn();
    const controller = new MotionCaptureController({
      nativeModule: module,
      now: () => 0,
      appState,
      timers: {
        setInterval: (handler, intervalMs) => setInterval(handler, intervalMs),
        clearInterval: clearIntervalSpy,
      },
    });

    await controller.initialize();
    await controller.startCapture();
    const subscription = module.subscriptions[0];

    controller.dispose();

    expect(subscription.removed).toBe(true);
    expect(module.stopCallCount).toBe(1);
    expect(clearIntervalSpy).toHaveBeenCalled();
  });

  it('background chama stop e retorno ao foreground nao reinicia automaticamente', async () => {
    const appState = new FakeAppState();
    const module = new FakeMotionModule();
    const controller = new MotionCaptureController({
      nativeModule: module,
      now: () => 0,
      appState,
    });

    await controller.initialize();
    await controller.startCapture();

    appState.emit('background');
    expect(module.stopCallCount).toBe(1);
    expect(controller.getSnapshot().status).toBe('stopped');

    appState.emit('active');
    expect(module.startCalls).toEqual([60]);
    expect(controller.getSnapshot().status).toBe('stopped');
  });

  it('background durante start pendente invalida resolve tardio', async () => {
    const appState = new FakeAppState();
    const module = new FakeMotionModule();
    module.startMode = 'deferred';
    const controller = new MotionCaptureController({
      nativeModule: module,
      now: () => 0,
      appState,
    });

    await controller.initialize();
    const startPromise = controller.startCapture();

    expect(controller.getSnapshot().status).toBe('starting');
    appState.emit('background');
    expect(controller.getSnapshot().status).toBe('stopped');
    expect(module.stopCallCount).toBe(1);

    module.pendingStart!.resolve({ sessionId: 'late-session', requestedRateHz: 60 });
    await startPromise;

    const snapshot = controller.getSnapshot();
    expect(snapshot.status).toBe('stopped');
    expect(snapshot.sessionId).toBeNull();
    expect(module.subscriptions).toHaveLength(0);
  });

  it('stop explicito durante start pendente invalida reject tardio', async () => {
    const module = new FakeMotionModule();
    module.startMode = 'deferred';
    const controller = new MotionCaptureController({
      nativeModule: module,
      now: () => 0,
      appState: new FakeAppState(),
    });

    await controller.initialize();
    const startPromise = controller.startCapture();
    controller.stopCapture('explicit');

    module.pendingStart!.reject(new Error('late failure'));
    await startPromise;

    const snapshot = controller.getSnapshot();
    expect(snapshot.status).toBe('stopped');
    expect(snapshot.errorMessage).toBeNull();
    expect(module.stopCallCount).toBe(1);
    expect(module.subscriptions).toHaveLength(0);
  });

  it('dispose durante start pendente remove subscription e ignora resolucao tardia', async () => {
    const module = new FakeMotionModule();
    module.startMode = 'deferred';
    const clearIntervalSpy = jest.fn();
    const controller = new MotionCaptureController({
      nativeModule: module,
      now: () => 0,
      appState: new FakeAppState(),
      timers: {
        setInterval: (handler, intervalMs) => setInterval(handler, intervalMs),
        clearInterval: clearIntervalSpy,
      },
    });

    await controller.initialize();
    const startPromise = controller.startCapture();
    controller.dispose();

    module.pendingStart!.resolve({ sessionId: 'late-session', requestedRateHz: 60 });
    await startPromise;

    expect(module.stopCallCount).toBe(1);
    expect(module.subscriptions).toHaveLength(0);
    expect(clearIntervalSpy).toHaveBeenCalled();
  });

  it('tentativa antiga nao sobrescreve captura mais nova', async () => {
    const module = new FakeMotionModule();
    module.startMode = 'deferred';
    const controller = new MotionCaptureController({
      nativeModule: module,
      now: () => 0,
      appState: new FakeAppState(),
    });

    await controller.initialize();
    const firstStart = controller.startCapture();
    const firstDeferred = module.pendingStart!;
    controller.stopCapture('explicit');

    const secondStart = controller.startCapture();
    const secondDeferred = module.pendingStart!;

    secondDeferred.resolve({ sessionId: 'new-session', requestedRateHz: 60 });
    await secondStart;

    firstDeferred.resolve({ sessionId: 'old-session', requestedRateHz: 60 });
    await firstStart;

    const snapshot = controller.getSnapshot();
    expect(snapshot.sessionId).toBe('new-session');
    expect(snapshot.status).toBe('starting');
    expect(module.stopCallCount).toBe(1);
  });

  it('fica stale sem primeira amostra e recupera ao receber frame valido', async () => {
    const module = new FakeMotionModule();
    let nowMs = 0;
    const controller = new MotionCaptureController({
      nativeModule: module,
      now: () => nowMs,
      appState: new FakeAppState(),
      staleThresholdMs: 250,
    });

    await controller.initialize();
    await controller.startCapture();

    nowMs = 260;
    jest.advanceTimersByTime(300);
    expect(controller.getSnapshot().status).toBe('stale');
    expect(controller.getSnapshot().lastSample).toBeNull();

    module.emit(buildFrame({ sequence: 0 }));
    expect(controller.getSnapshot().status).toBe('active');
    expect(controller.getSnapshot().lastSample?.sequence).toBe(0);
  });

  it('ignora start enquanto sensores ainda estao em verificacao', async () => {
    const module = new FakeMotionModule();
    const availabilityDeferred = createDeferred<SensorAvailability>();
    module.getAvailability = jest.fn(async () => availabilityDeferred.promise);
    const controller = new MotionCaptureController({
      nativeModule: module,
      now: () => 0,
      appState: new FakeAppState(),
    });

    const initializePromise = controller.initialize();
    expect(controller.getSnapshot().status).toBe('checking_sensors');

    await controller.startCapture();

    expect(module.startCalls).toHaveLength(0);
    expect(controller.getSnapshot().status).toBe('checking_sensors');

    availabilityDeferred.resolve({
      linearAcceleration: true,
      gravity: true,
      gyroscope: true,
    });
    await initializePromise;
  });

  it('modulo ausente apresenta estado nao suportado', async () => {
    const controller = new MotionCaptureController({
      nativeModule: null,
      now: () => 0,
      appState: new FakeAppState(),
    });

    await controller.initialize();

    expect(controller.getSnapshot().status).toBe('unsupported');
  });
});
