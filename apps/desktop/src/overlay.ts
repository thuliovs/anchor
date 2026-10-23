export type OverlayMode = "configuration" | "operation";
export type OverlayLifecycle = "closed" | "opening" | "open";

export interface TimeoutScheduler<Handle> {
  schedule(callback: () => void, delayMs: number): Handle;
  cancel(handle: Handle): void;
}

export const OVERLAY_COMMAND_TIMEOUT_MS = 10_000;

export interface OverlaySnapshotDto {
  supported: boolean;
  platform: string;
  lifecycle: OverlayLifecycle;
  isOpen: boolean;
  mode: OverlayMode;
  lastError: string | null;
}

export const OVERLAY_STATE_EVENT = "anchor-overlay-state-v1";

export const EMPTY_OVERLAY_SNAPSHOT: OverlaySnapshotDto = {
  supported: false,
  platform: "unknown",
  lifecycle: "closed",
  isOpen: false,
  mode: "configuration",
  lastError: null,
};

export function getOverlayModeLabel(mode: OverlayMode): string {
  return mode === "configuration"
    ? "Configuração (recebe mouse)"
    : "Operação (click-through)";
}

export function getOverlayStatusLabel(snapshot: OverlaySnapshotDto): string {
  if (!snapshot.supported) {
    return `Indisponível em ${snapshot.platform}`;
  }
  if (snapshot.lifecycle === "opening") {
    return "Abrindo…";
  }
  if (snapshot.lifecycle === "closed") {
    return "Fechado";
  }
  return getOverlayModeLabel(snapshot.mode);
}

export function withTimeout<T, Handle>(
  operation: Promise<T>,
  timeoutMs: number,
  scheduler: TimeoutScheduler<Handle>,
  timeoutMessage: string,
): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    let settled = false;
    const timeoutHandle = scheduler.schedule(() => {
      if (settled) return;
      settled = true;
      reject(new Error(timeoutMessage));
    }, timeoutMs);

    operation.then(
      (value) => {
        if (settled) return;
        settled = true;
        scheduler.cancel(timeoutHandle);
        resolve(value);
      },
      (error: unknown) => {
        if (settled) return;
        settled = true;
        scheduler.cancel(timeoutHandle);
        reject(error);
      },
    );
  });
}
