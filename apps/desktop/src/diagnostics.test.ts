/// <reference types="node" />

import test from "node:test";
import assert from "node:assert/strict";
import type { MotionSampleV1 } from "@anchor/protocol";
import {
  clamp,
  EMPTY_LIVE_TILT_SNAPSHOT,
  formatSampleNumber,
  getLiveTiltStateLabel,
  getNeutralReasonLabel,
  getDisplayedSample,
  mapAccelerationToOffset,
  RECEIVER_SOURCE_LABEL,
  usableTilt,
} from "./diagnostics";

function sample(overrides?: Partial<MotionSampleV1>): MotionSampleV1 {
  return {
    protocolVersion: 1,
    kind: "motion_sample",
    sessionId: "session-a",
    sequence: 1,
    sessionElapsedUs: 10_000,
    linearAccelerationMps2: { x: 0, y: 0, z: 0 },
    gravityMps2: { x: 0, y: 0, z: 9.81 },
    angularVelocityRadS: { x: 0, y: 0, z: 0 },
    ...overrides,
  };
}

test("clamp limits values to the requested range", () => {
  assert.equal(clamp(-5, -2, 2), -2);
  assert.equal(clamp(1.5, -2, 2), 1.5);
  assert.equal(clamp(9, -2, 2), 2);
});

test("mapAccelerationToOffset converts X and clamps displacement", () => {
  const offset = mapAccelerationToOffset(
    sample({ linearAccelerationMps2: { x: 18, y: 0, z: 0 } }),
    50,
    10,
  );

  assert.deepEqual(offset, { x: 50, y: 0 });
});

test("mapAccelerationToOffset inverts the vertical axis for CSS coordinates", () => {
  const offset = mapAccelerationToOffset(
    sample({ linearAccelerationMps2: { x: 0, y: 5, z: 0 } }),
    40,
    10,
  );

  assert.deepEqual(offset, { x: 0, y: -20 });
});

test("formatSampleNumber is safe when there is no sample", () => {
  assert.equal(formatSampleNumber(null, (current) => current.sequence), "--");
});

test("getDisplayedSample preserves the last sample while stale", () => {
  const lastSnapshotSample = sample({ sequence: 9 });

  assert.equal(getDisplayedSample("stale", null, lastSnapshotSample), lastSnapshotSample);
});

test("getDisplayedSample preserves the last sample while disconnected", () => {
  const lastSnapshotSample = sample({ sequence: 12 });

  assert.equal(
    getDisplayedSample("disconnected", null, lastSnapshotSample),
    lastSnapshotSample,
  );
});

test("receiver source label describes all IPv4 interfaces instead of loopback", () => {
  assert.equal(
    RECEIVER_SOURCE_LABEL,
    "Receptor UDP (todas as interfaces IPv4, porta 57421)",
  );
  assert.equal(RECEIVER_SOURCE_LABEL.includes("127.0.0.1"), false);
});

test("live tilt states map to user-facing labels", () => {
  assert.equal(getLiveTiltStateLabel("unavailable"), "Indisponível");
  assert.equal(getLiveTiltStateLabel("warming_up"), "Aquecendo");
  assert.equal(getLiveTiltStateLabel("valid"), "Válido");
  assert.equal(getLiveTiltStateLabel("invalid"), "Inválido");
  assert.equal(getLiveTiltStateLabel("stale"), "Sinal desatualizado");
  assert.equal(getLiveTiltStateLabel("disconnected"), "Desconectado");
});

test("neutral reason label replaces every underscore for ES2020 compatibility", () => {
  assert.equal(
    getNeutralReasonLabel("calibration_policy_provenance_mismatch"),
    "calibration policy provenance mismatch",
  );
});

test("usable tilt is neutral outside valid state", () => {
  assert.deepEqual(
    usableTilt({
      ...EMPTY_LIVE_TILT_SNAPSHOT,
      validity: "stale",
      targetTilt: { rollRad: 1, pitchRad: 2 },
    }),
    { rollRad: 0, pitchRad: 0 },
  );
});

test("usable tilt presents roll and pitch when valid", () => {
  assert.deepEqual(
    usableTilt({
      ...EMPTY_LIVE_TILT_SNAPSHOT,
      validity: "valid",
      neutralReason: "none",
      targetTilt: { rollRad: 0.1, pitchRad: -0.2 },
    }),
    { rollRad: 0.1, pitchRad: -0.2 },
  );
});

test("empty tilt snapshot has absent sample age", () => {
  assert.equal(EMPTY_LIVE_TILT_SNAPSHOT.lastSampleAgeMs, null);
});
