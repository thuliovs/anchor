import type { MotionSampleV1 } from "@anchor/protocol";

export const MOTION_SAMPLE_EVENT = "anchor-motion-sample-v1";
export const RECEIVER_SOURCE_LABEL = "Receptor UDP local (127.0.0.1:57421)";
export const SNAPSHOT_POLL_INTERVAL_MS = 250;
export const VISUAL_RANGE_MPS2 = 12;

export type ReceiverStatus = "active" | "stale" | "disconnected";

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
