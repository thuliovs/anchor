import {
  MAX_DATAGRAM_BYTES,
  PROTOCOL_VERSION,
  type MotionSampleV1,
  type Vector3,
} from '@anchor/protocol';

export type MotionPatternName = 'stationary' | 'sine';

type Range = {
  min: number;
  max: number;
};

export const MOTION_LIMITS: Record<
  'linearAccelerationMps2' | 'gravityMps2' | 'angularVelocityRadS',
  Range
> = {
  linearAccelerationMps2: { min: -200, max: 200 },
  gravityMps2: { min: -20, max: 20 },
  angularVelocityRadS: { min: -100, max: 100 },
};

const GRAVITY_Z_MPS2 = -9.80665;
const MICROS_PER_SECOND = 1_000_000;

const SINE_CONFIG = {
  lateralAccelerationAmplitude: 1.5,
  lateralAccelerationFrequencyHz: 0.5,
  longitudinalAccelerationAmplitude: 0.6,
  longitudinalAccelerationFrequencyHz: 0.25,
  yawVelocityAmplitude: 0.35,
  yawVelocityFrequencyHz: 0.4,
};

export type BuildMotionSampleOptions = {
  pattern: MotionPatternName;
  sessionId: string;
  sequence: number;
  sessionElapsedUs: number;
};

export function buildMotionSample(options: BuildMotionSampleOptions): MotionSampleV1 {
  const { pattern, sequence, sessionElapsedUs, sessionId } = options;

  const vectors = pattern === 'stationary'
    ? stationaryVectors()
    : sineVectors(sessionElapsedUs);

  return {
    protocolVersion: PROTOCOL_VERSION,
    kind: 'motion_sample',
    sessionId,
    sequence,
    sessionElapsedUs,
    ...vectors,
  };
}

export function serializeMotionSample(sample: MotionSampleV1): Buffer {
  return Buffer.from(JSON.stringify(sample), 'utf8');
}

export function payloadFitsDatagram(sample: MotionSampleV1): boolean {
  return serializeMotionSample(sample).byteLength <= MAX_DATAGRAM_BYTES;
}

export function isMotionSampleV1(value: unknown): value is MotionSampleV1 {
  if (typeof value !== 'object' || value === null) {
    return false;
  }

  const sample = value as Record<string, unknown>;
  return sample.protocolVersion === PROTOCOL_VERSION
    && sample.kind === 'motion_sample'
    && typeof sample.sessionId === 'string'
    && typeof sample.sequence === 'number'
    && Number.isInteger(sample.sequence)
    && sample.sequence >= 0
    && typeof sample.sessionElapsedUs === 'number'
    && Number.isSafeInteger(sample.sessionElapsedUs)
    && sample.sessionElapsedUs >= 0
    && isVector3(sample.linearAccelerationMps2)
    && isVector3(sample.gravityMps2)
    && isVector3(sample.angularVelocityRadS);
}

function stationaryVectors(): Pick<
  MotionSampleV1,
  'linearAccelerationMps2' | 'gravityMps2' | 'angularVelocityRadS'
> {
  return {
    linearAccelerationMps2: { x: 0, y: 0, z: 0 },
    gravityMps2: { x: 0, y: 0, z: GRAVITY_Z_MPS2 },
    angularVelocityRadS: { x: 0, y: 0, z: 0 },
  };
}

function sineVectors(sessionElapsedUs: number): Pick<
  MotionSampleV1,
  'linearAccelerationMps2' | 'gravityMps2' | 'angularVelocityRadS'
> {
  const timeSeconds = sessionElapsedUs / MICROS_PER_SECOND;

  return {
    linearAccelerationMps2: {
      x: round5(
        SINE_CONFIG.lateralAccelerationAmplitude
          * Math.sin(2 * Math.PI * SINE_CONFIG.lateralAccelerationFrequencyHz * timeSeconds),
      ),
      y: round5(
        SINE_CONFIG.longitudinalAccelerationAmplitude
          * Math.sin(2 * Math.PI * SINE_CONFIG.longitudinalAccelerationFrequencyHz * timeSeconds),
      ),
      z: 0,
    },
    gravityMps2: { x: 0, y: 0, z: GRAVITY_Z_MPS2 },
    angularVelocityRadS: {
      x: 0,
      y: 0,
      z: round5(
        SINE_CONFIG.yawVelocityAmplitude
          * Math.sin(2 * Math.PI * SINE_CONFIG.yawVelocityFrequencyHz * timeSeconds),
      ),
    },
  };
}

function isVector3(value: unknown): value is Vector3 {
  if (typeof value !== 'object' || value === null) {
    return false;
  }

  const vector = value as Record<string, unknown>;
  return typeof vector.x === 'number'
    && Number.isFinite(vector.x)
    && typeof vector.y === 'number'
    && Number.isFinite(vector.y)
    && typeof vector.z === 'number'
    && Number.isFinite(vector.z);
}

function round5(value: number): number {
  return Number(value.toFixed(5));
}
