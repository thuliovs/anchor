import test from 'node:test';
import assert from 'node:assert/strict';

import { MAX_DATAGRAM_BYTES, PROTOCOL_VERSION, type MotionSampleV1 } from '@anchor/protocol';

import {
  MOTION_LIMITS,
  buildMotionSample,
  isMotionSampleV1,
  serializeMotionSample,
} from '../src/patterns.js';

const SESSION_ID = 'sim-session-001';

function assertVectorWithinLimits(sample: MotionSampleV1): void {
  const vectors = [
    ['linearAccelerationMps2', sample.linearAccelerationMps2, MOTION_LIMITS.linearAccelerationMps2],
    ['gravityMps2', sample.gravityMps2, MOTION_LIMITS.gravityMps2],
    ['angularVelocityRadS', sample.angularVelocityRadS, MOTION_LIMITS.angularVelocityRadS],
  ] as const;

  for (const [, vector, range] of vectors) {
    assert.ok(vector.x >= range.min && vector.x <= range.max);
    assert.ok(vector.y >= range.min && vector.y <= range.max);
    assert.ok(vector.z >= range.min && vector.z <= range.max);
  }
}

test('stationary produces the expected values', () => {
  const sample = buildMotionSample({
    pattern: 'stationary',
    sequence: 0,
    sessionElapsedUs: 0,
    sessionId: SESSION_ID,
  });

  assert.deepEqual(sample, {
    protocolVersion: PROTOCOL_VERSION,
    kind: 'motion_sample',
    sessionId: SESSION_ID,
    sequence: 0,
    sessionElapsedUs: 0,
    linearAccelerationMps2: { x: 0, y: 0, z: 0 },
    gravityMps2: { x: 0, y: 0, z: -9.80665 },
    angularVelocityRadS: { x: 0, y: 0, z: 0 },
  });
});

test('sine is deterministic for a known monotonic instant', () => {
  const sample = buildMotionSample({
    pattern: 'sine',
    sequence: 12,
    sessionElapsedUs: 250_000,
    sessionId: SESSION_ID,
  });

  assert.equal(sample.protocolVersion, PROTOCOL_VERSION);
  assert.equal(sample.kind, 'motion_sample');
  assert.equal(sample.sequence, 12);
  assert.equal(sample.sessionElapsedUs, 250_000);
  assert.equal(sample.linearAccelerationMps2.x, 1.06066);
  assert.equal(sample.linearAccelerationMps2.y, 0.22961);
  assert.equal(sample.linearAccelerationMps2.z, 0);
  assert.equal(sample.gravityMps2.z, -9.80665);
  assert.equal(sample.angularVelocityRadS.z, 0.20572);
});

test('generated values stay within protocol limits', () => {
  const instants = [0, 125_000, 250_000, 500_000, 1_000_000, 2_000_000];

  for (const sessionElapsedUs of instants) {
    const sample = buildMotionSample({
      pattern: 'sine',
      sequence: 1,
      sessionElapsedUs,
      sessionId: SESSION_ID,
    });

    assertVectorWithinLimits(sample);
  }
});

test('generated object conforms to MotionSampleV1 shape', () => {
  const sample = buildMotionSample({
    pattern: 'sine',
    sequence: 4,
    sessionElapsedUs: 500_000,
    sessionId: SESSION_ID,
  });

  assert.ok(isMotionSampleV1(sample));
});

test('serialized payload stays below datagram limit', () => {
  const sample = buildMotionSample({
    pattern: 'sine',
    sequence: 42,
    sessionElapsedUs: 2_000_000,
    sessionId: SESSION_ID,
  });

  const payload = serializeMotionSample(sample);

  assert.ok(payload.byteLength <= MAX_DATAGRAM_BYTES);
});
