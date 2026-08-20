import React from 'react';
import ReactTestRenderer from 'react-test-renderer';

import { MotionCaptureController } from '../src/motion/MotionCaptureController';
import type { SensorAvailability } from '../src/motion/nativeMotionSensors';
import { AnchorSensorScreen } from '../src/screens/AnchorSensorScreen';

jest.mock('react-native-safe-area-context', () => {
  const { View } = require('react-native');

  return {
    SafeAreaView: ({ children }: { children: React.ReactNode }) => <View>{children}</View>,
  };
});

const appState = {
  addEventListener: () => ({
    remove: () => {},
  }),
};

const timers = {
  setInterval: () => 0 as unknown as ReturnType<typeof setInterval>,
  clearInterval: () => {},
};

function createDeferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(res => {
    resolve = res;
  });

  return { promise, resolve };
}

test('botao possui semantica de acessibilidade e a tela mostra o titulo', async () => {
  const controller = new MotionCaptureController({
    nativeModule: null,
    now: () => 0,
    appState,
    timers,
  });

  let renderer: ReactTestRenderer.ReactTestRenderer;
  await ReactTestRenderer.act(() => {
    renderer = ReactTestRenderer.create(<AnchorSensorScreen controller={controller} />);
  });

  const button = renderer!.root.find(
    node => node.props.accessibilityRole === 'button'
      && node.props.accessibilityLabel === 'Iniciar captura de sensores',
  );

  expect(button.props.accessibilityState).toEqual({ disabled: true, busy: false });
  expect(JSON.stringify(renderer!.toJSON())).toContain('Anchor Sensor');

  await ReactTestRenderer.act(() => {
    renderer!.unmount();
  });
});

test('botao inicial fica ocupado e desabilitado durante checking_sensors', async () => {
  const availabilityDeferred = createDeferred<SensorAvailability>();
  const controller = new MotionCaptureController({
    nativeModule: {
      getAvailability: async () => availabilityDeferred.promise,
      start: async () => ({ sessionId: 'session', requestedRateHz: 60 }),
      stop: () => {},
      onMotionFrame: () => ({ remove: () => {} } as never),
    },
    now: () => 0,
    appState,
    timers,
  });

  let renderer: ReactTestRenderer.ReactTestRenderer;
  await ReactTestRenderer.act(() => {
    renderer = ReactTestRenderer.create(<AnchorSensorScreen controller={controller} />);
  });

  const button = renderer!.root.find(
    node => node.props.accessibilityRole === 'button'
      && node.props.accessibilityLabel === 'Iniciar captura de sensores',
  );

  expect(button.props.accessibilityState).toEqual({ disabled: true, busy: true });
  expect(button.props.disabled).toBe(true);

  availabilityDeferred.resolve({
    linearAcceleration: true,
    gravity: true,
    gyroscope: true,
  });

  await ReactTestRenderer.act(async () => {
    await availabilityDeferred.promise;
  });

  await ReactTestRenderer.act(() => {
    renderer!.unmount();
  });
});
