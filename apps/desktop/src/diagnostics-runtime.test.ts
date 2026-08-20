/// <reference types="node" />

import assert from "node:assert/strict";
import test from "node:test";
import {
  createSequentialPoller,
  EMPTY_DIAGNOSTIC_ERRORS,
  setEventBridgeError,
  setSnapshotError,
  type SchedulerLike,
} from "./diagnostics-runtime";

type Deferred<T> = {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason?: unknown) => void;
};

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((innerResolve, innerReject) => {
    resolve = innerResolve;
    reject = innerReject;
  });

  return { promise, resolve, reject };
}

async function flushMicrotasks(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

function createFakeScheduler(): SchedulerLike<number> & {
  pendingCount: () => number;
  runNext: () => void;
} {
  let nextId = 1;
  const pending = new Map<number, () => void>();

  return {
    schedule(callback) {
      const handle = nextId++;
      pending.set(handle, callback);
      return handle;
    },
    cancel(handle) {
      pending.delete(handle);
    },
    pendingCount() {
      return pending.size;
    },
    runNext() {
      const next = pending.entries().next().value as [number, () => void] | undefined;
      if (next === undefined) {
        throw new Error("no scheduled callbacks");
      }

      const [handle, callback] = next;
      pending.delete(handle);
      callback();
    },
  };
}

test("sequential poller runs the first query immediately", async () => {
  let calls = 0;

  const poller = createSequentialPoller({
    intervalMs: 250,
    scheduler: createFakeScheduler(),
    poll: async () => {
      calls += 1;
      return calls;
    },
    onSuccess: () => {},
    onError: () => {},
  });

  await flushMicrotasks();
  assert.equal(calls, 1);
  poller.stop();
});

test("sequential poller never overlaps pending queries", async () => {
  const scheduler = createFakeScheduler();
  const first = deferred<number>();
  let calls = 0;

  const poller = createSequentialPoller({
    intervalMs: 250,
    scheduler,
    poll: async () => {
      calls += 1;
      return first.promise;
    },
    onSuccess: () => {},
    onError: () => {},
  });

  await flushMicrotasks();
  assert.equal(calls, 1);
  assert.equal(scheduler.pendingCount(), 0);

  first.resolve(1);
  await flushMicrotasks();
  assert.equal(calls, 1);
  assert.equal(scheduler.pendingCount(), 1);

  poller.stop();
});

test("sequential poller schedules the next query only after the previous one finishes", async () => {
  const scheduler = createFakeScheduler();
  const first = deferred<number>();
  let callIndex = 0;

  const poller = createSequentialPoller({
    intervalMs: 250,
    scheduler,
    poll: async () => {
      callIndex += 1;
      return callIndex === 1 ? first.promise : callIndex;
    },
    onSuccess: () => {},
    onError: () => {},
  });

  await flushMicrotasks();
  assert.equal(callIndex, 1);
  assert.equal(scheduler.pendingCount(), 0);

  first.resolve(1);
  await flushMicrotasks();
  assert.equal(scheduler.pendingCount(), 1);
  assert.equal(callIndex, 1);

  scheduler.runNext();
  await flushMicrotasks();
  assert.equal(callIndex, 2);

  poller.stop();
});

test("sequential poller continues after a rejection", async () => {
  const scheduler = createFakeScheduler();
  let callIndex = 0;
  const seenErrors: string[] = [];

  const poller = createSequentialPoller({
    intervalMs: 250,
    scheduler,
    poll: async () => {
      callIndex += 1;
      if (callIndex === 1) {
        throw new Error("boom");
      }

      return callIndex;
    },
    onSuccess: () => {},
    onError: (error) => {
      seenErrors.push(String(error));
    },
  });

  await flushMicrotasks();
  assert.equal(callIndex, 1);
  assert.deepEqual(seenErrors, ["Error: boom"]);
  assert.equal(scheduler.pendingCount(), 1);

  scheduler.runNext();
  await flushMicrotasks();
  assert.equal(callIndex, 2);

  poller.stop();
});

test("sequential poller stops future queries after cleanup", async () => {
  const scheduler = createFakeScheduler();
  let calls = 0;

  const poller = createSequentialPoller({
    intervalMs: 250,
    scheduler,
    poll: async () => {
      calls += 1;
      return calls;
    },
    onSuccess: () => {},
    onError: () => {},
  });

  await flushMicrotasks();
  assert.equal(calls, 1);
  assert.equal(scheduler.pendingCount(), 1);

  poller.stop();
  assert.equal(scheduler.pendingCount(), 0);

  await flushMicrotasks();
  assert.equal(calls, 1);
});

test("sequential poller ignores late resolution after cleanup", async () => {
  const scheduler = createFakeScheduler();
  const pending = deferred<number>();
  const seenResults: number[] = [];

  const poller = createSequentialPoller({
    intervalMs: 250,
    scheduler,
    poll: async () => pending.promise,
    onSuccess: (value) => {
      seenResults.push(value);
    },
    onError: () => {},
  });

  await flushMicrotasks();
  poller.stop();

  pending.resolve(42);
  await flushMicrotasks();

  assert.deepEqual(seenResults, []);
  assert.equal(scheduler.pendingCount(), 0);
});

test("diagnostic error state keeps event bridge and snapshot failures independent", () => {
  const withEventError = setEventBridgeError(
    EMPTY_DIAGNOSTIC_ERRORS,
    "listener failed",
  );
  const withBothErrors = setSnapshotError(withEventError, "snapshot failed");
  const clearedSnapshot = setSnapshotError(withBothErrors, null);
  const clearedEvent = setEventBridgeError(withBothErrors, null);

  assert.deepEqual(withBothErrors, {
    eventBridgeError: "listener failed",
    snapshotError: "snapshot failed",
  });
  assert.deepEqual(clearedSnapshot, {
    eventBridgeError: "listener failed",
    snapshotError: null,
  });
  assert.deepEqual(clearedEvent, {
    eventBridgeError: null,
    snapshotError: "snapshot failed",
  });
});
