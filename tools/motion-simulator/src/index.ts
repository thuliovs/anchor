import { createSocket } from 'node:dgram';
import { randomUUID } from 'node:crypto';
import { pathToFileURL } from 'node:url';

import { MAX_DATAGRAM_BYTES } from '@anchor/protocol';

import {
  type MotionPatternName,
  buildMotionSample,
  payloadFitsDatagram,
  serializeMotionSample,
} from './patterns.js';

type SimulatorOptions = {
  host: string;
  port: number;
  rate: number;
  pattern: MotionPatternName;
  durationSeconds?: number;
};

type SimulatorStats = {
  sentSamples: number;
  oversizedPayloads: number;
  startedAt: bigint;
  endedAt?: bigint;
};

type StopReason = 'duration' | 'signal' | 'error';

type RunSampleScheduleOptions = {
  rateHz: number;
  durationNs?: bigint;
  nowNs: () => bigint;
  sleepUntilNs: (deadlineNs: bigint) => Promise<void>;
  send: (sequence: number) => Promise<void>;
  isStopping: () => boolean;
};

type CreateSimulatorShutdownOptions = {
  requestStop: (reason: StopReason) => void;
  waitForScheduler?: () => Promise<void>;
  closeSocket: () => Promise<void>;
  markEndedAt: () => void;
  logSummary: (reason: StopReason) => void;
};

const DEFAULT_OPTIONS: SimulatorOptions = {
  host: '127.0.0.1',
  port: 57_421,
  rate: 60,
  pattern: 'sine',
};

const VALID_PATTERNS: MotionPatternName[] = ['stationary', 'sine'];

export async function main(): Promise<void> {
  const options = parseCliArgs(process.argv.slice(2));
  const socket = createSocket('udp4');
  const sessionId = randomUUID();
  const stats: SimulatorStats = {
    sentSamples: 0,
    oversizedPayloads: 0,
    startedAt: process.hrtime.bigint(),
  };
  let stopping = false;
  let stopReason: StopReason | undefined;
  let stopRequestedResolve: (() => void) | undefined;
  let activeTimer: NodeJS.Timeout | undefined;
  let activeSleepResolve: (() => void) | undefined;

  const stopRequested = new Promise<void>((resolve) => {
    stopRequestedResolve = resolve;
  });

  const requestStop = (reason: StopReason): void => {
    if (stopping) {
      return;
    }

    stopping = true;
    stopReason = reason;
    if (activeTimer) {
      clearTimeout(activeTimer);
      activeTimer = undefined;
    }
    if (activeSleepResolve) {
      activeSleepResolve();
      activeSleepResolve = undefined;
    }
    stopRequestedResolve?.();
  };

  const closeSocket = async (): Promise<void> => {
    await new Promise<void>((resolve) => {
      socket.close(() => resolve());
    });
  };

  const logSummary = (reason: StopReason): void => {
    const endedAt = stats.endedAt ?? process.hrtime.bigint();
    const elapsedUs = Number((endedAt - stats.startedAt) / 1_000n);
    const averageRateHz = elapsedUs > 0 ? (stats.sentSamples * 1_000_000) / elapsedUs : 0;
    console.log(
      `motion simulator summary: reason=${reason} host=${options.host} port=${options.port} pattern=${options.pattern} rate=${options.rate}Hz averageRateHz=${averageRateHz.toFixed(3)} sessionId=${sessionId} sent=${stats.sentSamples} oversized=${stats.oversizedPayloads} elapsedUs=${elapsedUs}`,
    );
  };

  console.log(
    `motion simulator sending to ${options.host}:${options.port} at ${options.rate}Hz with pattern=${options.pattern}`,
  );

  const sendSample = async (sequence: number): Promise<void> => {
    const now = process.hrtime.bigint();
    const sessionElapsedUs = Number((now - stats.startedAt) / 1_000n);
    const sample = buildMotionSample({
      pattern: options.pattern,
      sessionId,
      sequence,
      sessionElapsedUs,
    });

    if (!payloadFitsDatagram(sample)) {
      stats.oversizedPayloads += 1;
      throw new Error(`serialized payload exceeds ${MAX_DATAGRAM_BYTES} bytes`);
    }

    const payload = serializeMotionSample(sample);
    await new Promise<void>((resolve, reject) => {
      socket.send(payload, options.port, options.host, (error) => {
        if (error) {
          reject(error);
        } else {
          resolve();
        }
      });
    });

    stats.sentSamples += 1;
  };

  const sleepUntilNs = async (deadlineNs: bigint): Promise<void> => {
    while (!stopping) {
      const delayMs = computeRemainingDelayMs(process.hrtime.bigint(), deadlineNs);
      if (delayMs === 0) {
        return;
      }

      await new Promise<void>((resolve) => {
        activeSleepResolve = resolve;
        activeTimer = setTimeout(() => {
          activeTimer = undefined;
          activeSleepResolve = undefined;
          resolve();
        }, delayMs);
      });
    }
  };

  const durationNs = typeof options.durationSeconds === 'number'
    ? BigInt(Math.round(options.durationSeconds * 1_000_000_000))
    : undefined;

  const schedulerPromise = runSampleSchedule({
    rateHz: options.rate,
    durationNs,
    nowNs: () => process.hrtime.bigint(),
    sleepUntilNs,
    send: sendSample,
    isStopping: () => stopping,
  });
  const shutdown = createSimulatorShutdown({
    requestStop,
    waitForScheduler: async () => {
      await schedulerPromise;
    },
    closeSocket,
    markEndedAt: () => {
      stats.endedAt = process.hrtime.bigint();
    },
    logSummary,
  });

  process.once('SIGINT', () => {
    requestStop('signal');
  });

  try {
    await Promise.race([schedulerPromise, stopRequested]);
    if (!stopping) {
      requestStop('duration');
    }
    await shutdown(stopReason ?? 'duration');
  } catch (error: unknown) {
    console.error(`motion simulator send failure: ${formatError(error)}`);
    process.exitCode = 1;
    await shutdown('error');
  }
}

export function parseCliArgs(argv: string[]): SimulatorOptions {
  const options: SimulatorOptions = { ...DEFAULT_OPTIONS };

  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];

    if (flag === '--') {
      continue;
    }

    const value = argv[index + 1];

    if (!flag.startsWith('--')) {
      throw new Error(`unexpected argument: ${flag}`);
    }

    if (value === undefined) {
      throw new Error(`missing value for ${flag}`);
    }

    switch (flag) {
      case '--host':
        if (value.length === 0) {
          throw new Error('host must be a non-empty string');
        }
        options.host = value;
        break;
      case '--port':
        options.port = parseIntegerOption(value, 'port', 1, 65_535);
        break;
      case '--rate':
        options.rate = parseIntegerOption(value, 'rate', 1, 120);
        break;
      case '--pattern':
        if (!VALID_PATTERNS.includes(value as MotionPatternName)) {
          throw new Error(`pattern must be one of: ${VALID_PATTERNS.join(', ')}`);
        }
        options.pattern = value as MotionPatternName;
        break;
      case '--duration':
        options.durationSeconds = parseNumberOption(value, 'duration', 0.001);
        break;
      default:
        throw new Error(`unsupported option: ${flag}`);
    }

    index += 1;
  }

  return options;
}

export function parseIntegerOption(value: string, name: string, min: number, max: number): number {
  if (!/^\d+$/.test(value)) {
    throw new Error(`${name} must be an integer between ${min} and ${max}`);
  }

  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < min || parsed > max) {
    throw new Error(`${name} must be an integer between ${min} and ${max}`);
  }

  return parsed;
}

export function computeSampleDeadlineNs(
  startedAtNs: bigint,
  sequence: number,
  rateHz: number,
): bigint {
  return startedAtNs + (BigInt(sequence) * 1_000_000_000n) / BigInt(rateHz);
}

export function computeRemainingDelayMs(nowNs: bigint, deadlineNs: bigint): number {
  if (nowNs >= deadlineNs) {
    return 0;
  }

  const remainingNs = deadlineNs - nowNs;
  return Number((remainingNs + 999_999n) / 1_000_000n);
}

export function createMonotonicSleepUntil(
  nowNs: () => bigint,
  delayMs: (delayMs: number) => Promise<void>,
): (deadlineNs: bigint) => Promise<void> {
  return async (deadlineNs: bigint) => {
    const remainingDelayMs = computeRemainingDelayMs(nowNs(), deadlineNs);
    if (remainingDelayMs === 0) {
      return;
    }

    await delayMs(remainingDelayMs);
  };
}

export function createSimulatorShutdown({
  requestStop,
  waitForScheduler,
  closeSocket,
  markEndedAt,
  logSummary,
}: CreateSimulatorShutdownOptions): (reason: StopReason) => Promise<void> {
  let cleanupPromise: Promise<void> | undefined;

  return async (reason: StopReason) => {
    cleanupPromise ??= (async () => {
      requestStop(reason);
      if (waitForScheduler) {
        await Promise.allSettled([waitForScheduler()]);
      }
      markEndedAt();
      await closeSocket();
      logSummary(reason);
    })();

    await cleanupPromise;
  };
}

export async function runSampleSchedule({
  rateHz,
  durationNs,
  nowNs,
  sleepUntilNs,
  send,
  isStopping,
}: RunSampleScheduleOptions): Promise<void> {
  const startedAtNs = nowNs();
  const finalDeadlineNs = durationNs === undefined ? undefined : startedAtNs + durationNs;
  let sequence = 0;

  while (!isStopping()) {
    const deadlineNs = computeSampleDeadlineNs(startedAtNs, sequence, rateHz);
    if (finalDeadlineNs !== undefined && deadlineNs > finalDeadlineNs) {
      return;
    }

    if (sequence > 0) {
      await sleepUntilNs(deadlineNs);
    }

    if (isStopping()) {
      return;
    }

    await send(sequence);

    const minimalNextSequence = sequence + 1;
    const overdueSequence = Number(((nowNs() - startedAtNs) * BigInt(rateHz)) / 1_000_000_000n);
    sequence = Math.max(minimalNextSequence, overdueSequence);
  }
}

function parseNumberOption(value: string, name: string, minExclusive: number): number {
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed <= minExclusive) {
    throw new Error(`${name} must be greater than ${minExclusive}`);
  }

  return parsed;
}

function formatError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  void main().catch((error: unknown) => {
    console.error(`motion simulator failed: ${formatError(error)}`);
    process.exitCode = 1;
  });
}
