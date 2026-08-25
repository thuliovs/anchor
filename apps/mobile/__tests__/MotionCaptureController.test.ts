import type { EventSubscription } from 'react-native';

import { MotionCaptureController } from '../src/motion/MotionCaptureController';
import type {
  MotionSensorsModule,
  NativeMotionFrame,
  SensorAvailability,
} from '../src/motion/nativeMotionSensors';
import type { UdpSenderModule } from '../src/motion/nativeUdpSender';

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
  public startError: Error | null = null;

  async getAvailability(): Promise<SensorAvailability> {
    return this.availability;
  }

  async start(rateHz: number) {
    this.startCalls.push(rateHz);

    if (this.startError) {
      throw this.startError;
    }

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

class FakeUdpModule implements UdpSenderModule {
  public readonly openCalls: Array<{ host: string; port: number }> = [];
  public readonly sendCalls: string[] = [];
  public closeCallCount = 0;
  public openMode: 'immediate' | 'deferred' = 'immediate';
  public sendMode: 'immediate' | 'deferred' = 'immediate';
  public openError: Error | null = null;
  public sendError: Error | null = null;
  public pendingOpen: Deferred<void> | null = null;
  public pendingOpenDeferreds: Array<Deferred<void>> = [];
  public pendingSends: Array<Deferred<void>> = [];
  public isClosed = true;

  async open(host: string, port: number): Promise<void> {
    this.openCalls.push({ host, port });
    this.isClosed = false;

    if (this.openError) {
      throw this.openError;
    }

    if (this.openMode === 'deferred') {
      this.pendingOpen = createDeferred<void>();
      this.pendingOpenDeferreds.push(this.pendingOpen);
      return this.pendingOpen.promise;
    }
  }

  async send(payload: string): Promise<void> {
    if (this.isClosed) {
      throw new Error('transport closed');
    }

    this.sendCalls.push(payload);

    if (this.sendError) {
      throw this.sendError;
    }

    if (this.sendMode === 'deferred') {
      const deferred = createDeferred<void>();
      this.pendingSends.push(deferred);
      return deferred.promise;
    }
  }

  close(): void {
    this.closeCallCount += 1;
    this.isClosed = true;
  }
}

async function flushMicrotasks(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
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
      udpModule: new FakeUdpModule(),
      now: () => nowMs,
      appState,
    });

    await controller.initialize();

    expect(controller.getSnapshot().status).toBe('ready');
    expect(module.startCalls).toHaveLength(0);

    await controller.startCapture({ host: '192.168.0.20', port: 57421 });
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
      udpModule: new FakeUdpModule(),
      now: () => nowMs,
      appState,
    });

    await controller.initialize();
    await controller.startCapture({ host: '192.168.0.20', port: 57421 });

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
      udpModule: new FakeUdpModule(),
      now: () => 0,
      appState: new FakeAppState(),
    });

    await controller.initialize();
    await controller.startCapture({ host: '192.168.0.20', port: 57421 });

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
      udpModule: new FakeUdpModule(),
      now: () => 0,
      appState,
      timers: {
        setInterval: (handler, intervalMs) => setInterval(handler, intervalMs),
        clearInterval: clearIntervalSpy,
      },
    });

    await controller.initialize();
    await controller.startCapture({ host: '192.168.0.20', port: 57421 });
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
      udpModule: new FakeUdpModule(),
      now: () => 0,
      appState,
    });

    await controller.initialize();
    await controller.startCapture({ host: '192.168.0.20', port: 57421 });

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
      udpModule: new FakeUdpModule(),
      now: () => 0,
      appState,
    });

    await controller.initialize();
    const startPromise = controller.startCapture({ host: '192.168.0.20', port: 57421 });
    await flushMicrotasks();

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
      udpModule: new FakeUdpModule(),
      now: () => 0,
      appState: new FakeAppState(),
    });

    await controller.initialize();
    const startPromise = controller.startCapture({ host: '192.168.0.20', port: 57421 });
    await flushMicrotasks();
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
      udpModule: new FakeUdpModule(),
      now: () => 0,
      appState: new FakeAppState(),
      timers: {
        setInterval: (handler, intervalMs) => setInterval(handler, intervalMs),
        clearInterval: clearIntervalSpy,
      },
    });

    await controller.initialize();
    const startPromise = controller.startCapture({ host: '192.168.0.20', port: 57421 });
    await flushMicrotasks();
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
      udpModule: new FakeUdpModule(),
      now: () => 0,
      appState: new FakeAppState(),
    });

    await controller.initialize();
    const firstStart = controller.startCapture({ host: '192.168.0.20', port: 57421 });
    await flushMicrotasks();
    const firstDeferred = module.pendingStart!;
    controller.stopCapture('explicit');

    const secondStart = controller.startCapture({ host: '192.168.0.21', port: 57421 });
    await flushMicrotasks();
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
      udpModule: new FakeUdpModule(),
      now: () => nowMs,
      appState: new FakeAppState(),
      staleThresholdMs: 250,
    });

    await controller.initialize();
    await controller.startCapture({ host: '192.168.0.20', port: 57421 });

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
      udpModule: new FakeUdpModule(),
      now: () => 0,
      appState: new FakeAppState(),
    });

    const initializePromise = controller.initialize();
    expect(controller.getSnapshot().status).toBe('checking_sensors');

    await controller.startCapture({ host: '192.168.0.20', port: 57421 });

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
      udpModule: new FakeUdpModule(),
      now: () => 0,
      appState: new FakeAppState(),
    });

    await controller.initialize();

    expect(controller.getSnapshot().status).toBe('unsupported');
  });

  it('nao inicia sensores quando abrir o transporte falha', async () => {
    const module = new FakeMotionModule();
    const udpModule = new FakeUdpModule();
    udpModule.openError = new Error('cannot open udp');
    const controller = new MotionCaptureController({
      nativeModule: module,
      udpModule,
      now: () => 0,
      appState: new FakeAppState(),
    });

    await controller.initialize();
    await controller.startCapture({ host: '192.168.0.20', port: 57421 });

    expect(module.startCalls).toHaveLength(0);
    expect(module.subscriptions).toHaveLength(0);
    expect(udpModule.closeCallCount).toBe(1);
    expect(controller.getSnapshot().status).toBe('error');
  });

  it('fecha transporte se start dos sensores falhar depois da abertura', async () => {
    const module = new FakeMotionModule();
    module.startError = new Error('sensor start failed');
    const udpModule = new FakeUdpModule();
    const controller = new MotionCaptureController({
      nativeModule: module,
      udpModule,
      now: () => 0,
      appState: new FakeAppState(),
    });

    await controller.initialize();
    await controller.startCapture({ host: '192.168.0.20', port: 57421 });

    expect(module.startCalls).toEqual([60]);
    expect(udpModule.openCalls).toEqual([{ host: '192.168.0.20', port: 57421 }]);
    expect(udpModule.closeCallCount).toBe(1);
    expect(module.subscriptions).toHaveLength(0);
    expect(controller.getSnapshot().status).toBe('error');
  });

  it('erro de envio encerra captura de forma controlada e preserva ultimo erro', async () => {
    const module = new FakeMotionModule();
    const udpModule = new FakeUdpModule();
    udpModule.sendError = new Error('send failed');
    const controller = new MotionCaptureController({
      nativeModule: module,
      udpModule,
      now: () => 0,
      appState: new FakeAppState(),
    });

    await controller.initialize();
    await controller.startCapture({ host: '192.168.0.20', port: 57421 });
    module.emit(buildFrame({ sequence: 0 }));
    await flushMicrotasks();

    expect(module.stopCallCount).toBe(1);
    expect(udpModule.closeCallCount).toBe(1);
    expect(controller.getSnapshot().status).toBe('error');
    expect(controller.getSnapshot().errorMessage).toContain('send failed');
  });

  it('stop explicito com envio pendente descarta pendencia e ignora resolve tardio', async () => {
    const module = new FakeMotionModule();
    const udpModule = new FakeUdpModule();
    udpModule.sendMode = 'deferred';
    const controller = new MotionCaptureController({
      nativeModule: module,
      udpModule,
      now: () => 0,
      appState: new FakeAppState(),
    });

    await controller.initialize();
    await controller.startCapture({ host: '192.168.0.20', port: 57421 });
    module.emit(buildFrame({ sequence: 0 }));
    await flushMicrotasks();

    controller.stopCapture('explicit');
    udpModule.pendingSends[0].resolve();
    await flushMicrotasks();

    expect(controller.getSnapshot().status).toBe('stopped');
    expect(controller.getSnapshot().transportMetrics.sentDatagrams).toBe(0);
  });

  it('background com envio pendente descarta pendencia e nao reinicia no resume', async () => {
    const appState = new FakeAppState();
    const module = new FakeMotionModule();
    const udpModule = new FakeUdpModule();
    udpModule.sendMode = 'deferred';
    const controller = new MotionCaptureController({
      nativeModule: module,
      udpModule,
      now: () => 0,
      appState,
    });

    await controller.initialize();
    await controller.startCapture({ host: '192.168.0.20', port: 57421 });
    module.emit(buildFrame({ sequence: 0 }));
    await flushMicrotasks();

    appState.emit('background');
    udpModule.pendingSends[0].resolve();
    await flushMicrotasks();
    appState.emit('active');

    expect(controller.getSnapshot().status).toBe('stopped');
    expect(module.startCalls).toEqual([60]);
  });

  it('dispose com envio pendente fecha transporte e ignora resolucao tardia', async () => {
    const module = new FakeMotionModule();
    const udpModule = new FakeUdpModule();
    udpModule.sendMode = 'deferred';
    const controller = new MotionCaptureController({
      nativeModule: module,
      udpModule,
      now: () => 0,
      appState: new FakeAppState(),
    });

    await controller.initialize();
    await controller.startCapture({ host: '192.168.0.20', port: 57421 });
    module.emit(buildFrame({ sequence: 0 }));
    await flushMicrotasks();

    controller.dispose();
    udpModule.pendingSends[0].resolve();
    await flushMicrotasks();

    expect(udpModule.closeCallCount).toBeGreaterThanOrEqual(1);
    expect(module.stopCallCount).toBe(1);
  });

  it('open UDP lento nao marca stale antes do start dos sensores', async () => {
    const module = new FakeMotionModule();
    const udpModule = new FakeUdpModule();
    udpModule.openMode = 'deferred';
    let nowMs = 0;
    const controller = new MotionCaptureController({
      nativeModule: module,
      udpModule,
      now: () => nowMs,
      appState: new FakeAppState(),
      staleThresholdMs: 250,
    });

    await controller.initialize();
    const startPromise = controller.startCapture({ host: '192.168.0.20', port: 57421 });
    await flushMicrotasks();

    nowMs = 400;
    jest.advanceTimersByTime(400);

    expect(controller.getSnapshot().status).toBe('starting');
    expect(controller.getSnapshot().transportState).toBe('opening');
    expect(module.startCalls).toHaveLength(0);

    udpModule.pendingOpen!.resolve();
    await startPromise;

    expect(module.startCalls).toEqual([60]);
    expect(controller.getSnapshot().status).toBe('starting');
  });
});
