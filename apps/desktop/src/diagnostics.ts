import type { MotionSampleV1 } from "@anchor/protocol";

export const MOTION_SAMPLE_EVENT = "anchor-motion-sample-v1";
export const RECEIVER_SOURCE_LABEL = "Receptor UDP (todas as interfaces IPv4, porta 57421)";
export const SNAPSHOT_POLL_INTERVAL_MS = 250;
export const VISUAL_RANGE_MPS2 = 12;

export type ReceiverStatus = "active" | "stale" | "disconnected";
export type LiveTiltState =
  | "unavailable"
  | "awaiting_sample"
  | "warming_up"
  | "valid"
  | "invalid"
  | "stale"
  | "disconnected";
export type LiveTiltNeutralReason =
  | "none"
  | "missing_configuration"
  | "invalid_profile"
  | "invalid_policy"
  | "calibration_policy_provenance_mismatch"
  | "awaiting_first_sample"
  | "warm_up"
  | "session_changed"
  | "temporal_gap"
  | "invalid_dt"
  | "sequence_non_monotonic"
  | "calibration_error"
  | "estimator_error"
  | "stale"
  | "disconnected";

export interface ReceiverSnapshotDto {
  status: ReceiverStatus;
  lastSample: MotionSampleV1 | null;
  activeSender: string | null;
  activeSessionId: string | null;
  lastSequence: number | null;
  lastValidAgeMs: number | null;
  metrics: ReceiverMetricsDto;
}

export interface ReceiverMetricsDto {
  receivedDatagrams: number;
  acceptedSamples: number;
  oversizedDatagrams: number;
  invalidPackets: number;
  duplicateOrOutOfOrderPackets: number;
  foreignSessionPackets: number;
  rateLimitedDatagrams: number;
}

export interface Tilt2Dto {
  rollRad: number;
  pitchRad: number;
}

export interface AngleEstimateDto extends Tilt2Dto {
  yawAvailable: boolean;
  gravityDirection: { x: number; y: number; z: number };
}

export interface LiveTiltMetricsDto {
  processedEvents: number;
  validOutputs: number;
  sequenceGapEvents: number;
  estimatedMissingSamples: number;
  nonMonotonicSequenceEvents: number;
  gapResets: number;
  invalidDtResets: number;
  sessionResets: number;
  calibrationErrors: number;
  estimatorErrors: number;
  provenanceMismatches: number;
}

export interface LiveTiltIntegrationMetricsDto {
  ingressDroppedEvents: number;
  ingressClosedEvents: number;
  processedEvents: number;
}

export interface CalibrationProfileFingerprintDto {
  version: number;
  algorithm: string;
  digest: string;
}

export interface LiveTiltSnapshotDto {
  snapshotVersion: number;
  validity: LiveTiltState;
  neutralReason: LiveTiltNeutralReason;
  targetTilt: Tilt2Dto;
  lastEstimate: AngleEstimateDto | null;
  activeSessionId: string | null;
  lastSequence: number | null;
  lastSampleAgeMs: number | null;
  yawAvailable: boolean;
  calibrationProfileFingerprint: CalibrationProfileFingerprintDto | null;
  policyVersion: number | null;
  metrics: LiveTiltMetricsDto;
  lastProcessingError: string | null;
  integrationMetrics: LiveTiltIntegrationMetricsDto;
}

export const EMPTY_LIVE_TILT_SNAPSHOT: LiveTiltSnapshotDto = {
  snapshotVersion: 1,
  validity: "unavailable",
  neutralReason: "missing_configuration",
  targetTilt: { rollRad: 0, pitchRad: 0 },
  lastEstimate: null,
  activeSessionId: null,
  lastSequence: null,
  lastSampleAgeMs: null,
  yawAvailable: false,
  calibrationProfileFingerprint: null,
  policyVersion: null,
  metrics: {
    processedEvents: 0,
    validOutputs: 0,
    sequenceGapEvents: 0,
    estimatedMissingSamples: 0,
    nonMonotonicSequenceEvents: 0,
    gapResets: 0,
    invalidDtResets: 0,
    sessionResets: 0,
    calibrationErrors: 0,
    estimatorErrors: 0,
    provenanceMismatches: 0,
  },
  lastProcessingError: null,
  integrationMetrics: {
    ingressDroppedEvents: 0,
    ingressClosedEvents: 0,
    processedEvents: 0,
  },
};

export interface VisualOffset {
  x: number;
  y: number;
}

export const EMPTY_RECEIVER_SNAPSHOT: ReceiverSnapshotDto = {
  status: "disconnected",
  lastSample: null,
  activeSender: null,
  activeSessionId: null,
  lastSequence: null,
  lastValidAgeMs: null,
  metrics: {
    receivedDatagrams: 0,
    acceptedSamples: 0,
    oversizedDatagrams: 0,
    invalidPackets: 0,
    duplicateOrOutOfOrderPackets: 0,
    foreignSessionPackets: 0,
    rateLimitedDatagrams: 0,
  },
};

export function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}

export function mapAccelerationToOffset(
  sample: MotionSampleV1 | null,
  maxDistancePx: number,
  maxAccelerationMps2 = VISUAL_RANGE_MPS2,
): VisualOffset {
  if (sample === null) {
    return { x: 0, y: 0 };
  }

  const normalizedX = clamp(
    sample.linearAccelerationMps2.x / maxAccelerationMps2,
    -1,
    1,
  );
  const normalizedY = clamp(
    sample.linearAccelerationMps2.y / maxAccelerationMps2,
    -1,
    1,
  );

  const x = normalizedX * maxDistancePx;
  const y = normalizedY * -maxDistancePx;

  return {
    x: Object.is(x, -0) ? 0 : x,
    y: Object.is(y, -0) ? 0 : y,
  };
}

export function getDisplayedSample(
  status: ReceiverStatus,
  liveSample: MotionSampleV1 | null,
  snapshotLastSample: MotionSampleV1 | null,
): MotionSampleV1 | null {
  if (status === "active") {
    return liveSample ?? snapshotLastSample;
  }

  return snapshotLastSample ?? liveSample;
}

export function formatNumber(
  value: number | null | undefined,
  fractionDigits = 2,
): string {
  return typeof value === "number" && Number.isFinite(value)
    ? value.toFixed(fractionDigits)
    : "--";
}

export function formatText(value: string | null | undefined): string {
  return value !== null && value !== undefined && value.length > 0 ? value : "--";
}

export function formatAge(value: number | null | undefined): string {
  return value === null || value === undefined ? "--" : `${value} ms`;
}

export function formatSampleNumber(
  sample: MotionSampleV1 | null,
  selector: (sample: MotionSampleV1) => number,
  fractionDigits = 2,
): string {
  return sample === null ? "--" : formatNumber(selector(sample), fractionDigits);
}

export function getStatusLabel(status: ReceiverStatus): string {
  switch (status) {
    case "active":
      return "Ativo";
    case "stale":
      return "Sinal desatualizado";
    case "disconnected":
      return "Aguardando sinal";
  }
}

export function getLiveTiltStateLabel(state: LiveTiltState): string {
  switch (state) {
    case "unavailable":
      return "Indisponível";
    case "awaiting_sample":
      return "Aguardando amostra";
    case "warming_up":
      return "Aquecendo";
    case "valid":
      return "Válido";
    case "invalid":
      return "Inválido";
    case "stale":
      return "Sinal desatualizado";
    case "disconnected":
      return "Desconectado";
  }
}

export function getNeutralReasonLabel(reason: LiveTiltNeutralReason): string {
  return reason === "none" ? "Nenhum" : reason.replace(/_/g, " ");
}

export function usableTilt(snapshot: LiveTiltSnapshotDto): Tilt2Dto {
  return snapshot.validity === "valid"
    ? snapshot.targetTilt
    : { rollRad: 0, pitchRad: 0 };
}

export function shortFingerprint(
  value: CalibrationProfileFingerprintDto | null | undefined,
): string {
  return value ? value.digest.slice(0, 12) : "--";
}
