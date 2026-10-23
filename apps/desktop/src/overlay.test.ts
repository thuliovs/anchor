/// <reference types="node" />

import assert from "node:assert/strict";
import test from "node:test";
import {
  EMPTY_OVERLAY_SNAPSHOT,
  getOverlayModeLabel,
  getOverlayStatusLabel,
  withTimeout,
  type TimeoutScheduler,
} from "./overlay";

type Deferred<T> = {
  promise: Promise<T>;
  resolve: (value: T) => void;
};

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((innerResolve) => {
    resolve = innerResolve;
  });
  return { promise, resolve };
}

function fakeTimeoutScheduler(): TimeoutScheduler<number> & { expire: () => void } {
  let callback: (() => void) | null = null;
  return {
    schedule(next) {
      callback = next;
      return 1;
    },
    cancel() {
      callback = null;
    },
    expire() {
      const pending = callback;
      callback = null;
      pending?.();
    },
  };
}

test("closed and unsupported overlay states have explicit labels", () => {
  assert.equal(getOverlayStatusLabel(EMPTY_OVERLAY_SNAPSHOT), "Indisponível em unknown");
  assert.equal(
    getOverlayStatusLabel({
      ...EMPTY_OVERLAY_SNAPSHOT,
      supported: true,
      platform: "windows",
    }),
    "Fechado",
  );
});

test("overlay modes distinguish input behavior", () => {
  assert.equal(getOverlayModeLabel("configuration"), "Configuração (recebe mouse)");
  assert.equal(getOverlayModeLabel("operation"), "Operação (click-through)");
});

test("opening lifecycle is explicit before the window is ready", () => {
  assert.equal(
    getOverlayStatusLabel({
      ...EMPTY_OVERLAY_SNAPSHOT,
      supported: true,
      platform: "windows",
      lifecycle: "opening",
    }),
    "Abrindo…",
  );
});

test("overlay command timeout rejects and does not wait forever", async () => {
  const pending = deferred<string>();
  const scheduler = fakeTimeoutScheduler();
  const guarded = withTimeout(pending.promise, 10_000, scheduler, "backend timeout");

  scheduler.expire();
  await assert.rejects(guarded, /backend timeout/);

  // A late backend result is consumed but cannot change the timeout result.
  pending.resolve("late result");
  await Promise.resolve();
});

test("overlay command response cancels its timeout", async () => {
  const pending = deferred<string>();
  const scheduler = fakeTimeoutScheduler();
  const guarded = withTimeout(pending.promise, 10_000, scheduler, "backend timeout");

  pending.resolve("opened");
  assert.equal(await guarded, "opened");
  scheduler.expire();
});
