import test from 'node:test';
import assert from 'node:assert/strict';

import {
  computeSampleDeadlineNs,
  createMonotonicSleepUntil,
  createSimulatorShutdown,
  runSampleSchedule,
} from '../src/index.js';

test('deadlines are correct for 60 Hz', () => {
  const startNs = 0n;

  assert.equal(computeSampleDeadlineNs(startNs, 0, 60), 0n);
  assert.equal(computeSampleDeadlineNs(startNs, 1, 60), 16_666_666n);
  assert.equal(computeSampleDeadlineNs(startNs, 2, 60), 33_333_333n);
  assert.equal(computeSampleDeadlineNs(startNs, 3, 60), 50_000_000n);
});

test('deadline calculation does not accumulate drift', () => {
  const startNs = 0n;
  const lastDeadline = computeSampleDeadlineNs(startNs, 300, 60);

  assert.equal(lastDeadline, 5_000_000_000n);
});

test('schedule keeps sequence strictly increasing', async () => {
  let nowNs = 0n;
  const sequences: number[] = [];

  await runSampleSchedule({
    rateHz: 60,
    durationNs: 50_000_000n,
    nowNs: () => nowNs,
    sleepUntilNs: async (deadlineNs) => {
      nowNs = deadlineNs;
    },
    send: async (sequence) => {
      sequences.push(sequence);
      nowNs += 1_000n;
    },
    isStopping: () => false,
  });

  assert.deepEqual(sequences, [0, 1, 2, 3]);
});

test('schedule sends nothing new after shutdown', async () => {
  let nowNs = 0n;
  let sentCount = 0;
  let stopping = false;

  await runSampleSchedule({
    rateHz: 60,
    durationNs: 1_000_000_000n,
    nowNs: () => nowNs,
    sleepUntilNs: async (deadlineNs) => {
      nowNs = deadlineNs;
    },
    send: async () => {
      sentCount += 1;
      stopping = true;
    },
    isStopping: () => stopping,
  });

  assert.equal(sentCount, 1);
});

test('schedule does not allow overlapping sends', async () => {
  let nowNs = 0n;
  let inFlight = false;
  let maxConcurrent = 0;

  await runSampleSchedule({
    rateHz: 60,
    durationNs: 50_000_000n,
    nowNs: () => nowNs,
    sleepUntilNs: async (deadlineNs) => {
      nowNs = deadlineNs;
    },
    send: async () => {
      assert.equal(inFlight, false);
      inFlight = true;
      maxConcurrent += 1;
      await Promise.resolve();
      inFlight = false;
    },
    isStopping: () => false,
  });

  assert.equal(maxConcurrent, 4);
});

test('sleep helper waits only for the remaining monotonic time', async () => {
  const observedDelaysMs: number[] = [];
  const sleepUntil = createMonotonicSleepUntil(
    () => 1_000_000n,
    async (delayMs) => {
      observedDelaysMs.push(delayMs);
    },
  );

  await sleepUntil(2_500_000n);
  await sleepUntil(900_000n);

  assert.deepEqual(observedDelaysMs, [2]);
});

test('shutdown cleanup runs once even when the scheduler rejects', async () => {
  let requestStopCalls = 0;
  let closedSockets = 0;
  let summaries = 0;
  const schedulerError = new Error('send failed');
  const schedulerPromise = Promise.reject(schedulerError);
  const stats = {
    sentSamples: 0,
    oversizedPayloads: 0,
    startedAt: 0n,
    endedAt: undefined as bigint | undefined,
  };

  const shutdown = createSimulatorShutdown({
    requestStop: () => {
      requestStopCalls += 1;
    },
    waitForScheduler: async () => {
      await schedulerPromise;
    },
    closeSocket: async () => {
      closedSockets += 1;
    },
    markEndedAt: () => {
      stats.endedAt = 1_000n;
    },
    logSummary: () => {
      summaries += 1;
    },
  });

  await shutdown('error');
  await shutdown('error');

  assert.equal(requestStopCalls, 1);
  assert.equal(closedSockets, 1);
  assert.equal(summaries, 1);
  assert.equal(stats.endedAt, 1_000n);
});
