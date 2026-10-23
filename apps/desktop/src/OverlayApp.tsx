import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import {
  EMPTY_OVERLAY_SNAPSHOT,
  getOverlayModeLabel,
  OVERLAY_STATE_EVENT,
  type OverlayMode,
  type OverlaySnapshotDto,
} from "./overlay";
import "./App.css";

function OverlayApp() {
  const [snapshot, setSnapshot] = useState<OverlaySnapshotDto>(EMPTY_OVERLAY_SNAPSHOT);
  const [pointer, setPointer] = useState({ x: 0, y: 0 });
  const [commandError, setCommandError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;

    void invoke<OverlaySnapshotDto>("get_overlay_snapshot")
      .then((next) => {
        if (!disposed) setSnapshot(next);
      })
      .catch((error) => {
        if (!disposed) setCommandError(String(error));
      });

    void listen<OverlaySnapshotDto>(OVERLAY_STATE_EVENT, (event) => {
      if (!disposed) setSnapshot(event.payload);
    })
      .then((cleanup) => {
        if (disposed) cleanup();
        else unlisten = cleanup;
      })
      .catch((error) => {
        if (!disposed) setCommandError(`Eventos do overlay indisponíveis: ${String(error)}`);
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const setMode = async (mode: OverlayMode) => {
    try {
      setCommandError(null);
      setSnapshot(await invoke<OverlaySnapshotDto>("set_overlay_mode", { mode }));
    } catch (error) {
      setCommandError(String(error));
    }
  };

  const close = async () => {
    try {
      setCommandError(null);
      setSnapshot(await invoke<OverlaySnapshotDto>("close_overlay"));
    } catch (error) {
      setCommandError(String(error));
    }
  };

  return (
    <main
      className={`overlay overlay--${snapshot.mode}`}
      onPointerMove={(event) => setPointer({ x: event.clientX, y: event.clientY })}
    >
      <div className="overlay__frame" aria-hidden="true" />
      <div className="overlay__crosshair" aria-hidden="true" />
      <section className="overlay__badge">
        <strong>ANCHOR OVERLAY C1 · PROVISÓRIO</strong>
        <span>{getOverlayModeLabel(snapshot.mode)}</span>
        {snapshot.mode === "configuration" ? (
          <>
            <span>Mouse: {pointer.x}, {pointer.y}</span>
            <div className="overlay__actions">
              <button type="button" onClick={() => void setMode("operation")}>
                Ativar click-through
              </button>
              <button type="button" className="button--danger" onClick={() => void close()}>
                Fechar overlay
              </button>
            </div>
          </>
        ) : (
          <span>Recupere pelo ícone do Anchor na bandeja.</span>
        )}
        {commandError ?? snapshot.lastError ? (
          <span className="overlay__error">{commandError ?? snapshot.lastError}</span>
        ) : null}
      </section>
    </main>
  );
}

export default OverlayApp;
