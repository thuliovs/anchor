export const PROTOCOL_VERSION = 1 as const;
export const MAX_DATAGRAM_BYTES = 1024 as const;

/**
 * Vehicle-calibrated coordinate system:
 * X+ = right, Y+ = forward, Z+ = up.
 */
export interface Vector3 {
  x: number;
  y: number;
  z: number;
}

/**
 * Monotonic session-relative motion sample for protocol v1.
 *
 * `sessionId` is an opaque session identifier and is not authentication.
 * `sessionElapsedUs` is measured from the session start and is not wall-clock time.
 * All vectors use the vehicle-calibrated coordinate system:
 * X+ = right, Y+ = forward, Z+ = up.
 */
export interface MotionSampleV1 {
  protocolVersion: typeof PROTOCOL_VERSION;
  kind: 'motion_sample';
  sessionId: string;
  sequence: number;
  sessionElapsedUs: number;
  linearAccelerationMps2: Vector3;
  gravityMps2: Vector3;
  angularVelocityRadS: Vector3;
}
