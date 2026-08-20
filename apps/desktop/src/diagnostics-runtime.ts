export interface DiagnosticErrorState {
  eventBridgeError: string | null;
  snapshotError: string | null;
}

export interface SchedulerLike<Handle> {
  schedule(callback: () => void, delayMs: number): Handle;
  cancel(handle: Handle): void;
}

export interface SequentialPollerOptions<Handle, Result> {
  intervalMs: number;
  scheduler: SchedulerLike<Handle>;
  poll: () => Promise<Result>;
  onSuccess: (result: Result) => void;
  onError: (error: unknown) => void;
}

export const EMPTY_DIAGNOSTIC_ERRORS: DiagnosticErrorState = {
  eventBridgeError: null,
  snapshotError: null,
};

export function setEventBridgeError(
  current: DiagnosticErrorState,
  eventBridgeError: string | null,
): DiagnosticErrorState {
  return {
    ...current,
    eventBridgeError,
  };
}

export function setSnapshotError(
  current: DiagnosticErrorState,
  snapshotError: string | null,
): DiagnosticErrorState {
  return {
    ...current,
    snapshotError,
  };
}

export function createSequentialPoller<Handle, Result>(
  options: SequentialPollerOptions<Handle, Result>,
): { stop: () => void } {
  let isStopped = false;
  let scheduledHandle: Handle | null = null;

  const scheduleNext = () => {
    if (isStopped) {
      return;
    }

    scheduledHandle = options.scheduler.schedule(() => {
      scheduledHandle = null;
      void runCycle();
    }, options.intervalMs);
  };

  const runCycle = async () => {
    try {
      const result = await options.poll();
      if (!isStopped) {
        options.onSuccess(result);
      }
    } catch (error) {
      if (!isStopped) {
        options.onError(error);
      }
    } finally {
      scheduleNext();
    }
  };

  void runCycle();

  return {
    stop() {
      isStopped = true;
      if (scheduledHandle !== null) {
        options.scheduler.cancel(scheduledHandle);
        scheduledHandle = null;
      }
    },
  };
}
