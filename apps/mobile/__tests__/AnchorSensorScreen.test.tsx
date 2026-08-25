import React from 'react';
import ReactTestRenderer from 'react-test-renderer';

import type { MotionCaptureController } from '../src/motion/MotionCaptureController';
import type {
  MotionCaptureSnapshot,
} from '../src/motion/MotionCaptureController';
import { AnchorSensorScreen } from '../src/screens/AnchorSensorScreen';

jest.mock('react-native-safe-area-context', () => {
  const { View } = require('react-native');

  return {
    SafeAreaView: ({ children }: { children: React.ReactNode }) => <View>{children}</View>,
  };
});

function makeSnapshot(
  overrides: Partial<MotionCaptureSnapshot> = {},
): MotionCaptureSnapshot {
  return {
    status: 'ready',
    availability: {
      linearAcceleration: true,
      gravity: true,
      gyroscope: true,
    },
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
    transportState: 'idle',
    transportDestination: null,
    transportMetrics: {
      offeredDatagrams: 0,
      sentDatagrams: 0,
      droppedBackpressure: 0,
      rejectedPayloads: 0,
      sendErrors: 0,
      lastTransportError: null,
    },
    ...overrides,
  };
}

class FakeScreenController {
  public snapshot: MotionCaptureSnapshot;
  public readonly startCapture = jest.fn(async () => undefined);
  public readonly stopCapture = jest.fn(() => undefined);
  public readonly initialize = jest.fn(async () => undefined);
  public readonly dispose = jest.fn(() => undefined);
  private listener: ((snapshot: MotionCaptureSnapshot) => void) | null = null;

  constructor(snapshot: MotionCaptureSnapshot) {
    this.snapshot = snapshot;
  }

  subscribe(listener: (snapshot: MotionCaptureSnapshot) => void) {
    this.listener = listener;
    listener(this.snapshot);
    return () => {
      if (this.listener === listener) {
        this.listener = null;
      }
    };
  }

  getSnapshot() {
    return this.snapshot;
  }

  setSnapshot(snapshot: MotionCaptureSnapshot) {
    this.snapshot = snapshot;
    this.listener?.(snapshot);
  }
}

test('botao possui semantica de acessibilidade e a tela mostra o titulo', async () => {
  const controller = new FakeScreenController(makeSnapshot({
    status: 'unsupported',
    availability: null,
    errorMessage: 'Turbo Native Modules indisponiveis neste ambiente.',
  }));

  let renderer: ReactTestRenderer.ReactTestRenderer;
  await ReactTestRenderer.act(async () => {
    renderer = ReactTestRenderer.create(
      <AnchorSensorScreen controller={controller as unknown as MotionCaptureController} />,
    );
  });

  const button = renderer!.root.find(
    node => node.props.accessibilityRole === 'button'
      && node.props.accessibilityLabel === 'Iniciar captura de sensores',
  );

  expect(button.props.accessibilityState).toEqual({ disabled: true, busy: false });
  expect(JSON.stringify(renderer!.toJSON())).toContain('Anchor Sensor');
  expect(JSON.stringify(renderer!.toJSON())).toContain('IP do computador');
  expect(JSON.stringify(renderer!.toJSON())).toContain('UDP nao confirma se o computador recebeu os dados');

  await ReactTestRenderer.act(() => {
    renderer!.unmount();
  });
});

test('botao inicial fica ocupado e desabilitado durante checking_sensors', async () => {
  const controller = new FakeScreenController(makeSnapshot({
    status: 'checking_sensors',
    availability: null,
  }));

  let renderer: ReactTestRenderer.ReactTestRenderer;
  await ReactTestRenderer.act(async () => {
    renderer = ReactTestRenderer.create(
      <AnchorSensorScreen controller={controller as unknown as MotionCaptureController} />,
    );
  });

  const button = renderer!.root.find(
    node => node.props.accessibilityRole === 'button'
      && node.props.accessibilityLabel === 'Iniciar captura de sensores',
  );

  expect(button.props.accessibilityState).toEqual({ disabled: true, busy: false });
  expect(button.props.disabled).toBe(true);

  await ReactTestRenderer.act(() => {
    renderer!.unmount();
  });
});

test('campos de destino comecam invalidos e bloqueiam inicio', async () => {
  const controller = new FakeScreenController(makeSnapshot());

  let renderer: ReactTestRenderer.ReactTestRenderer;
  await ReactTestRenderer.act(async () => {
    renderer = ReactTestRenderer.create(
      <AnchorSensorScreen controller={controller as unknown as MotionCaptureController} />,
    );
  });

  const tree = JSON.stringify(renderer!.toJSON());
  const button = renderer!.root.find(
    node => node.props.accessibilityRole === 'button'
      && node.props.accessibilityLabel === 'Iniciar captura de sensores',
  );

  expect(tree).toContain('Porta');
  expect(tree).toContain('57421');
  expect(button.props.disabled).toBe(true);

  await ReactTestRenderer.act(() => {
    renderer!.unmount();
  });
});

test('destino nao promete confirmacao de recebimento e alerta sobre loopback', async () => {
  const controller = new FakeScreenController(makeSnapshot({
    status: 'unsupported',
    availability: null,
  }));

  let renderer: ReactTestRenderer.ReactTestRenderer;
  await ReactTestRenderer.act(async () => {
    renderer = ReactTestRenderer.create(
      <AnchorSensorScreen controller={controller as unknown as MotionCaptureController} />,
    );
  });

  const hostInput = renderer!.root.findByProps({ accessibilityLabel: 'IP do computador' });

  await ReactTestRenderer.act(async () => {
    hostInput.props.onChangeText('127.0.0.1');
  });

  const tree = JSON.stringify(renderer!.toJSON());

  expect(tree).not.toContain('conectado ao PC');
  expect(tree).toContain('127.0.0.1');
  expect(tree).toContain('aponta para o proprio celular');

  await ReactTestRenderer.act(() => {
    renderer!.unmount();
  });
});
