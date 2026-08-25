import { PROTOCOL_VERSION, type MotionSampleV1 } from '@anchor/protocol';

import {
  convertNativeMotionFrameToSample,
  assertUtf8DatagramSize,
  measureUtf8ByteLength,
  serializeMotionSampleV1Datagram,
} from '../src/motion/motionSample';
import { validateUdpDestination } from '../src/motion/udpDestination';
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

function buildSample(overrides: Partial<MotionSampleV1> = {}): MotionSampleV1 {
  return {
    protocolVersion: PROTOCOL_VERSION,
    kind: 'motion_sample',
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

  it('serializa sample valido para JSON UTF-8 aceito pelo protocolo', () => {
    const converted = convertNativeMotionFrameToSample(buildFrame());

    expect(converted.ok).toBe(true);
    if (!converted.ok) {
      throw new Error('expected valid sample');
    }

    const datagram = serializeMotionSampleV1Datagram(converted.sample);

    expect(datagram.payload).toBe(
      JSON.stringify({
        protocolVersion: PROTOCOL_VERSION,
        kind: 'motion_sample',
        sessionId: 'session-123',
        sequence: 7,
        sessionElapsedUs: 45_000,
        linearAccelerationMps2: { x: 1, y: -2, z: 3 },
        gravityMps2: { x: 0, y: 0, z: -9.80665 },
        angularVelocityRadS: { x: 0.1, y: 0.2, z: 0.3 },
      }),
    );
    expect(datagram.byteLength).toBe(measureUtf8ByteLength(datagram.payload));
  });

  it('mede corretamente bytes UTF-8 multibyte no sessionId', () => {
    const converted = convertNativeMotionFrameToSample(buildFrame({ sessionId: 'sessao-celular-ae-漢字' }));

    expect(converted.ok).toBe(true);
    if (!converted.ok) {
      throw new Error('expected valid sample');
    }

    const datagram = serializeMotionSampleV1Datagram(converted.sample);

    expect(datagram.byteLength).toBe(measureUtf8ByteLength(datagram.payload));
    expect(datagram.payload).toContain('sessao-celular-ae-漢字');
  });

  it('rejeita sessionId vazio durante serializacao', () => {
    expect(() => serializeMotionSampleV1Datagram(buildSample({ sessionId: '' }))).toThrow(/sessionId/i);
  });

  it('rejeita sessionId acima de 64 caracteres durante serializacao', () => {
    expect(() => serializeMotionSampleV1Datagram(buildSample({ sessionId: 'a'.repeat(65) }))).toThrow(/sessionId/i);
  });

  it('rejeita sequence acima de uint32 durante serializacao', () => {
    expect(() => serializeMotionSampleV1Datagram(buildSample({ sequence: 4_294_967_296 }))).toThrow(/sequence/i);
  });

  it('rejeita elapsed fora de safe integer durante serializacao', () => {
    expect(() => serializeMotionSampleV1Datagram(buildSample({ sessionElapsedUs: Number.MAX_SAFE_INTEGER + 1 }))).toThrow(/sessionElapsedUs/i);
  });

  it('rejeita vetores fora dos limites fisicos durante serializacao', () => {
    expect(() => serializeMotionSampleV1Datagram(buildSample({ gravityMps2: { x: 0, y: 0, z: -25 } }))).toThrow(/gravityMps2/i);
  });

  it('rejeita valores nao finitos antes da serializacao', () => {
    const invalidConversion = convertNativeMotionFrameToSample(
      buildFrame({ gravityMps2: { x: 0, y: Number.NaN, z: -9.80665 } }),
    );

    expect(invalidConversion).toEqual({ ok: false, reason: 'invalid_vector' });

    expect(() => serializeMotionSampleV1Datagram({
      protocolVersion: PROTOCOL_VERSION,
      kind: 'motion_sample',
      sessionId: 'session-123',
      sequence: 7,
      sessionElapsedUs: 45_000,
      linearAccelerationMps2: { x: 1, y: -2, z: 3 },
      gravityMps2: { x: 0, y: Number.NaN, z: -9.80665 },
      angularVelocityRadS: { x: 0.1, y: 0.2, z: 0.3 },
    })).toThrow(/finite/i);
  });

  it('serializa exatamente um datagrama por sample', () => {
    const converted = convertNativeMotionFrameToSample(buildFrame());

    expect(converted.ok).toBe(true);
    if (!converted.ok) {
      throw new Error('expected valid sample');
    }

    const datagram = serializeMotionSampleV1Datagram(converted.sample);

    expect(typeof datagram.payload).toBe('string');
    expect(datagram.payload.startsWith('{')).toBe(true);
    expect(datagram.payload.endsWith('}')).toBe(true);
    expect(datagram.byteLength).toBeGreaterThan(0);
  });
});

describe('UTF-8 datagram helpers', () => {
  it('mede payload com exatamente 1024 bytes', () => {
    const payload = 'a'.repeat(1024);

    expect(measureUtf8ByteLength(payload)).toBe(1024);
    expect(() => assertUtf8DatagramSize(payload)).not.toThrow();
  });

  it('rejeita payload com 1025 bytes', () => {
    const payload = 'a'.repeat(1025);

    expect(measureUtf8ByteLength(payload)).toBe(1025);
    expect(() => assertUtf8DatagramSize(payload)).toThrow(/1024 bytes/i);
  });
});

describe('validateUdpDestination', () => {
  it('gera alerta para qualquer loopback 127.0.0.0/8', () => {
    expect(validateUdpDestination('127.0.0.1', 57421).loopbackWarning).toMatch(/proprio celular/i);
    expect(validateUdpDestination('127.12.34.56', 57421).loopbackWarning).toMatch(/proprio celular/i);
  });

  it('rejeita 0.0.0.0/8 multicast e faixa reservada 240.0.0.0/4', () => {
    expect(validateUdpDestination('0.0.0.1', 57421).ok).toBe(false);
    expect(validateUdpDestination('224.0.0.1', 57421).ok).toBe(false);
    expect(validateUdpDestination('239.1.2.3', 57421).ok).toBe(false);
    expect(validateUdpDestination('240.0.0.1', 57421).ok).toBe(false);
  });
});
