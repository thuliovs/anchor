import { PROTOCOL_VERSION, type MotionSampleV1 } from '@anchor/protocol';

import {
  convertNativeMotionFrameToSample,
} from '../src/motion/motionSample';
import type { NativeMotionFrame } from '../src/motion/nativeMotionSensors';

function buildFrame(overrides: Partial<NativeMotionFrame> = {}): NativeMotionFrame {
  return {
    sessionId: 'session-123',
    sequence: 7,
    sessionElapsedUs: 45_000,
    linearAccelerationMps2: { x: 1, y: -2, z: 3 },
    gravityMps2: { x: 0, y: 0, z: -9.80665 },
    angularVelocityRadS: { x: 0.1, y: 0.2, z: 0.3 },
    ...overrides,
  };
}

describe('convertNativeMotionFrameToSample', () => {
  it('converte frame valido para MotionSampleV1', () => {
    const result = convertNativeMotionFrameToSample(buildFrame());

    expect(result.ok).toBe(true);
    if (!result.ok) {
      throw new Error('expected valid sample');
    }

    const sample: MotionSampleV1 = result.sample;
    expect(sample.protocolVersion).toBe(PROTOCOL_VERSION);
    expect(sample.kind).toBe('motion_sample');
    expect(sample.sequence).toBe(7);
    expect(sample.gravityMps2.z).toBeCloseTo(-9.80665, 5);
  });

  it('inclui protocolVersion e kind', () => {
    const result = convertNativeMotionFrameToSample(buildFrame());

    expect(result.ok).toBe(true);
    if (!result.ok) {
      throw new Error('expected valid sample');
    }

    expect(result.sample).toMatchObject({
      protocolVersion: PROTOCOL_VERSION,
      kind: 'motion_sample',
    });
  });

  it('rejeita NaN e Infinity', () => {
    const nanResult = convertNativeMotionFrameToSample(
      buildFrame({ linearAccelerationMps2: { x: Number.NaN, y: 0, z: 0 } }),
    );
    const infinityResult = convertNativeMotionFrameToSample(
      buildFrame({ angularVelocityRadS: { x: 0, y: Number.POSITIVE_INFINITY, z: 0 } }),
    );

    expect(nanResult).toEqual({ ok: false, reason: 'invalid_vector' });
    expect(infinityResult).toEqual({ ok: false, reason: 'invalid_vector' });
  });

  it('rejeita limites fisicos fora do protocolo', () => {
    const result = convertNativeMotionFrameToSample(
      buildFrame({ gravityMps2: { x: 0, y: 0, z: -25 } }),
    );

    expect(result).toEqual({ ok: false, reason: 'out_of_range' });
  });

  it('rejeita sequencia invalida', () => {
    const fractional = convertNativeMotionFrameToSample(buildFrame({ sequence: 1.5 }));
    const negative = convertNativeMotionFrameToSample(buildFrame({ sequence: -1 }));

    expect(fractional).toEqual({ ok: false, reason: 'invalid_sequence' });
    expect(negative).toEqual({ ok: false, reason: 'invalid_sequence' });
  });
});
