import {
  MAX_DATAGRAM_BYTES,
  PROTOCOL_VERSION,
  type MotionSampleV1,
  type Vector3,
} from '@anchor/protocol';

import type { NativeMotionFrame } from './nativeMotionSensors';

type ValidationReason =
  | 'invalid_structure'
  | 'invalid_session'
  | 'invalid_sequence'
  | 'invalid_elapsed'
  | 'invalid_vector'
  | 'out_of_range';

export type ConversionResult =
  | { ok: true; sample: MotionSampleV1 }
  | { ok: false; reason: ValidationReason };

export interface SerializedMotionSampleDatagram {
  payload: string;
  byteLength: number;
}

const UINT32_MAX = 4_294_967_295;
const SAFE_INTEGER_MAX = Number.MAX_SAFE_INTEGER;

const VECTOR_LIMITS = {
  linearAccelerationMps2: { min: -200, max: 200 },
  gravityMps2: { min: -20, max: 20 },
  angularVelocityRadS: { min: -100, max: 100 },
} as const;

export function convertNativeMotionFrameToSample(
  frame: NativeMotionFrame,
): ConversionResult {
  if (!isNativeMotionFrame(frame)) {
    return { ok: false, reason: 'invalid_structure' };
  }

  if (frame.sessionId.length === 0 || frame.sessionId.length > 64) {
    return { ok: false, reason: 'invalid_session' };
  }

  if (!isUint32(frame.sequence)) {
    return { ok: false, reason: 'invalid_sequence' };
  }

  if (!Number.isSafeInteger(frame.sessionElapsedUs) || frame.sessionElapsedUs < 0) {
    return { ok: false, reason: 'invalid_elapsed' };
  }

  if (!isFiniteVector(frame.linearAccelerationMps2)
    || !isFiniteVector(frame.gravityMps2)
    || !isFiniteVector(frame.angularVelocityRadS)) {
    return { ok: false, reason: 'invalid_vector' };
  }

  if (!isWithinRange(frame.linearAccelerationMps2, VECTOR_LIMITS.linearAccelerationMps2)
    || !isWithinRange(frame.gravityMps2, VECTOR_LIMITS.gravityMps2)
    || !isWithinRange(frame.angularVelocityRadS, VECTOR_LIMITS.angularVelocityRadS)) {
    return { ok: false, reason: 'out_of_range' };
  }

  return {
    ok: true,
    sample: {
      protocolVersion: PROTOCOL_VERSION,
      kind: 'motion_sample',
      sessionId: frame.sessionId,
      sequence: frame.sequence,
      sessionElapsedUs: frame.sessionElapsedUs,
      linearAccelerationMps2: frame.linearAccelerationMps2,
      gravityMps2: frame.gravityMps2,
      angularVelocityRadS: frame.angularVelocityRadS,
    },
  };
}

export function serializeMotionSampleV1Datagram(
  sample: MotionSampleV1,
): SerializedMotionSampleDatagram {
  const normalizedSample: MotionSampleV1 = {
    protocolVersion: sample.protocolVersion,
    kind: sample.kind,
    sessionId: sample.sessionId,
    sequence: sample.sequence,
    sessionElapsedUs: sample.sessionElapsedUs,
    linearAccelerationMps2: sample.linearAccelerationMps2,
    gravityMps2: sample.gravityMps2,
    angularVelocityRadS: sample.angularVelocityRadS,
  };

  const payload = JSON.stringify(normalizedSample);
  validateSerializableMotionSample(normalizedSample);
  const byteLength = assertUtf8DatagramSize(payload);

  return { payload, byteLength };
}

export function measureUtf8ByteLength(value: string): number {
  let byteLength = 0;

  for (let index = 0; index < value.length; index += 1) {
    const codePoint = value.codePointAt(index);
    if (codePoint === undefined) {
      continue;
    }

    if (codePoint <= 0x7F) {
      byteLength += 1;
    } else if (codePoint <= 0x7FF) {
      byteLength += 2;
    } else if (codePoint <= 0xFFFF) {
      byteLength += 3;
    } else {
      byteLength += 4;
      index += 1;
    }
  }

  return byteLength;
}

export function assertUtf8DatagramSize(value: string): number {
  const byteLength = measureUtf8ByteLength(value);
  if (byteLength > MAX_DATAGRAM_BYTES) {
    throw new Error(`Serialized MotionSampleV1 exceeds ${MAX_DATAGRAM_BYTES} bytes.`);
  }

  return byteLength;
}

function isNativeMotionFrame(value: unknown): value is NativeMotionFrame {
  if (typeof value !== 'object' || value === null) {
    return false;
  }

  const frame = value as Record<string, unknown>;
  return typeof frame.sessionId === 'string'
    && typeof frame.sequence === 'number'
    && typeof frame.sessionElapsedUs === 'number'
    && isVectorLike(frame.linearAccelerationMps2)
    && isVectorLike(frame.gravityMps2)
    && isVectorLike(frame.angularVelocityRadS);
}

function isVectorLike(value: unknown): value is Vector3 {
  if (typeof value !== 'object' || value === null) {
    return false;
  }

  const vector = value as Record<string, unknown>;
  return typeof vector.x === 'number'
    && typeof vector.y === 'number'
    && typeof vector.z === 'number';
}

function isFiniteVector(vector: Vector3): boolean {
  return Number.isFinite(vector.x)
    && Number.isFinite(vector.y)
    && Number.isFinite(vector.z);
}

function validateSerializableMotionSample(sample: MotionSampleV1): void {
  if (sample.protocolVersion !== PROTOCOL_VERSION) {
    throw new Error(`protocolVersion must be ${PROTOCOL_VERSION}.`);
  }

  if (sample.kind !== 'motion_sample') {
    throw new Error('kind must be motion_sample.');
  }

  const sessionIdLength = Array.from(sample.sessionId).length;
  if (sessionIdLength === 0 || sessionIdLength > 64) {
    throw new Error('sessionId must contain between 1 and 64 characters.');
  }

  if (!isUint32(sample.sequence)) {
    throw new Error(`sequence must be a uint32 between 0 and ${UINT32_MAX}.`);
  }

  if (!Number.isSafeInteger(sample.sessionElapsedUs) || sample.sessionElapsedUs < 0) {
    throw new Error('sessionElapsedUs must be a non-negative safe integer.');
  }

  validateSerializableVector('linearAccelerationMps2', sample.linearAccelerationMps2, VECTOR_LIMITS.linearAccelerationMps2);
  validateSerializableVector('gravityMps2', sample.gravityMps2, VECTOR_LIMITS.gravityMps2);
  validateSerializableVector('angularVelocityRadS', sample.angularVelocityRadS, VECTOR_LIMITS.angularVelocityRadS);
}

function validateSerializableVector(
  fieldName: string,
  vector: Vector3,
  range: { min: number; max: number },
): void {
  if (!isFiniteVector(vector)) {
    throw new Error(`${fieldName} must contain only finite numeric components.`);
  }

  if (!isWithinRange(vector, range)) {
    throw new Error(`${fieldName} must stay within ${range.min}..${range.max}.`);
  }
}

function isWithinRange(
  vector: Vector3,
  range: { min: number; max: number },
): boolean {
  return isWithin(vector.x, range.min, range.max)
    && isWithin(vector.y, range.min, range.max)
    && isWithin(vector.z, range.min, range.max);
}

function isWithin(value: number, min: number, max: number): boolean {
  return value >= min && value <= max;
}

function isUint32(value: number): boolean {
  return Number.isInteger(value) && value >= 0 && value <= UINT32_MAX && value <= SAFE_INTEGER_MAX;
}
