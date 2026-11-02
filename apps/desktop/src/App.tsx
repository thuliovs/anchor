import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import type { MotionSampleV1 } from "@anchor/protocol";
import {
  createSequentialPoller,
  EMPTY_DIAGNOSTIC_ERRORS,
  setEventBridgeError,
  setSnapshotError,
  setTiltSnapshotError,
  type DiagnosticErrorState,
} from "./diagnostics-runtime";
import {
  EMPTY_RECEIVER_SNAPSHOT,
  EMPTY_LIVE_TILT_SNAPSHOT,
  MOTION_SAMPLE_EVENT,
  RECEIVER_SOURCE_LABEL,
  SNAPSHOT_POLL_INTERVAL_MS,
  formatAge,
  formatNumber,
  formatSampleNumber,
  formatText,
  getDisplayedSample,
  getStatusLabel,
  getLiveTiltStateLabel,
  getNeutralReasonLabel,
  mapAccelerationToOffset,
  shortFingerprint,
  usableTilt,
  type LiveTiltSnapshotDto,
  type ReceiverSnapshotDto,
} from "./diagnostics";
import "./App.css";
import {
  EMPTY_OVERLAY_SNAPSHOT,
  getOverlayStatusLabel,
  OVERLAY_COMMAND_TIMEOUT_MS,
  OVERLAY_STATE_EVENT,
  withTimeout,
  type OverlayMode,
  type OverlaySnapshotDto,
} from "./overlay";

const VISUAL_RADIUS_PX = 108;

function App() {
  const [snapshot, setSnapshot] =
    useState<ReceiverSnapshotDto>(EMPTY_RECEIVER_SNAPSHOT);
  const [liveSample, setLiveSample] = useState<MotionSampleV1 | null>(null);
  const [tiltSnapshot, setTiltSnapshot] =
    useState<LiveTiltSnapshotDto>(EMPTY_LIVE_TILT_SNAPSHOT);
  const [errors, setErrors] =
    useState<DiagnosticErrorState>(EMPTY_DIAGNOSTIC_ERRORS);
  const [overlay, setOverlay] = useState<OverlaySnapshotDto>(EMPTY_OVERLAY_SNAPSHOT);
  const [overlayPending, setOverlayPending] = useState(false);
  const [overlayCommandError, setOverlayCommandError] = useState<string | null>(null);
  const [overlayReadError, setOverlayReadError] = useState<string | null>(null);

  useEffect(() => {
    let isDisposed = false;
    let unlisten: (() => void) | null = null;

    const poller = createSequentialPoller({
      intervalMs: SNAPSHOT_POLL_INTERVAL_MS,
      scheduler: {
        schedule: (callback, delayMs) => window.setTimeout(callback, delayMs),
        cancel: (handle) => window.clearTimeout(handle),
      },
      poll: async () => {
        const result: {
          nextReceiverSnapshot?: ReceiverSnapshotDto;
          nextTiltSnapshot?: LiveTiltSnapshotDto;
          nextOverlaySnapshot?: OverlaySnapshotDto;
          receiverError: string | null;
          tiltError: string | null;
          overlayError: string | null;
        } = { receiverError: null, tiltError: null, overlayError: null };
        try {
          result.nextReceiverSnapshot = await invoke<ReceiverSnapshotDto>(
            "get_receiver_snapshot",
          );
        } catch (error) {
          result.receiverError = `Snapshot indisponível: ${String(error)}`;
        }
        try {
          result.nextTiltSnapshot = await invoke<LiveTiltSnapshotDto>(
            "get_live_tilt_snapshot",
          );
        } catch (error) {
          result.tiltError = `Tilt processado indisponível: ${String(error)}`;
        }
        try {
          result.nextOverlaySnapshot = await invoke<OverlaySnapshotDto>(
            "get_overlay_snapshot",
          );
        } catch (error) {
          result.overlayError = `Estado do overlay indisponível: ${String(error)}`;
        }
        return result;
      },
      onSuccess: ({
        nextReceiverSnapshot,
        nextTiltSnapshot,
        nextOverlaySnapshot,
        receiverError,
        tiltError,
        overlayError,
      }) => {
        if (isDisposed) {
          return;
        }

        if (nextReceiverSnapshot) {
          setSnapshot(nextReceiverSnapshot);
        }
        if (nextTiltSnapshot) {
          setTiltSnapshot(nextTiltSnapshot);
        }
        if (nextOverlaySnapshot) {
          setOverlay(nextOverlaySnapshot);
        }
        setOverlayReadError(overlayError);
        setErrors((current) =>
          setTiltSnapshotError(setSnapshotError(current, receiverError), tiltError),
        );
      },
      onError: (error) => {
        if (isDisposed) {
          return;
        }

        setErrors((current) =>
          setSnapshotError(current, `Snapshot indisponível: ${String(error)}`),
        );
      },
    });

    const setupBridge = async () => {
      try {
        unlisten = await listen<MotionSampleV1>(MOTION_SAMPLE_EVENT, (event) => {
          if (isDisposed) {
            return;
          }

          setLiveSample(event.payload);
        });

        if (isDisposed) {
          unlisten();
          unlisten = null;
          return;
        }

        setErrors((current) => setEventBridgeError(current, null));
      } catch (error) {
        if (!isDisposed) {
          setErrors((current) =>
            setEventBridgeError(
              current,
              `Ponte Tauri indisponível: ${String(error)}`,
            ),
          );
        }
      }
    };

    void setupBridge();

    return () => {
      isDisposed = true;
      poller.stop();

      if (unlisten !== null) {
        unlisten();
      }
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void listen<OverlaySnapshotDto>(OVERLAY_STATE_EVENT, (event) => {
      if (!disposed) setOverlay(event.payload);
    })
      .then((cleanup) => {
        if (disposed) cleanup();
        else unlisten = cleanup;
      })
      .catch((error) => {
        if (!disposed) {
          setOverlayReadError(`Eventos do overlay indisponíveis: ${String(error)}`);
        }
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const runOverlayCommand = async (
    command: "open_overlay" | "close_overlay" | "set_overlay_mode",
    mode?: OverlayMode,
  ) => {
    setOverlayPending(true);
    setOverlayCommandError(null);
    try {
      const next = await withTimeout(
        invoke<OverlaySnapshotDto>(
          command,
          mode === undefined ? undefined : { mode },
        ),
        OVERLAY_COMMAND_TIMEOUT_MS,
        {
          schedule: (callback, delayMs) => window.setTimeout(callback, delayMs),
          cancel: (handle) => window.clearTimeout(handle),
        },
        `O backend não respondeu ao comando ${command} em ${OVERLAY_COMMAND_TIMEOUT_MS / 1000} segundos. Os controles foram reabilitados; use “Fechar overlay” ou a bandeja para recuperação.`,
      );
      setOverlay(next);
    } catch (error) {
      setOverlayCommandError(`Falha no overlay: ${String(error)}`);
    } finally {
      setOverlayPending(false);
    }
  };

  const displayedSample = getDisplayedSample(
    snapshot.status,
    liveSample,
    snapshot.lastSample,
  );
  const markerOffset = mapAccelerationToOffset(displayedSample, VISUAL_RADIUS_PX);
  const targetTilt = usableTilt(tiltSnapshot);

  return (
    <main className={`app app--${snapshot.status}`}>
      <header className="app__header">
        <div>
          <p className="eyebrow">Entrada atual: {RECEIVER_SOURCE_LABEL}</p>
          <h1>Anchor - Diagnóstico de Movimento</h1>
        </div>
        <div className={`status-pill status-pill--${snapshot.status}`}>
          <span className="status-pill__dot" aria-hidden="true" />
          {getStatusLabel(snapshot.status)}
        </div>
      </header>

      {errors.eventBridgeError ? (
        <p className="bridge-error">{errors.eventBridgeError}</p>
      ) : null}
      {errors.snapshotError ? (
        <p className="bridge-error">{errors.snapshotError}</p>
      ) : null}
      {errors.tiltSnapshotError ? (
        <p className="bridge-error">{errors.tiltSnapshotError}</p>
      ) : null}
      {overlayCommandError || overlayReadError || overlay.lastError ? (
        <p className="bridge-error">
          {overlayCommandError ?? overlayReadError ?? overlay.lastError}
        </p>
      ) : null}

      <section className="overlay-controls panel" aria-labelledby="overlay-controls-title">
        <div className="panel__heading">
          <h2 id="overlay-controls-title">Overlay experimental C1</h2>
          <p>Estado mantido pelo backend Rust; sem conexão com targetTilt nesta fatia.</p>
        </div>
        <div className="overlay-controls__status">
          <strong>{getOverlayStatusLabel(overlay)}</strong>
          <span>
            {overlay.lifecycle === "opening"
              ? "Criação em andamento"
              : overlay.isOpen
                ? "Janela ativa"
                : "Janela inativa"}
          </span>
        </div>
        <div className="overlay-controls__actions">
          <button
            type="button"
            disabled={overlayPending || !overlay.supported || overlay.lifecycle === "open"}
            onClick={() => void runOverlayCommand("open_overlay")}
          >
            Abrir overlay
          </button>
          <button
            type="button"
            disabled={
              overlayPending || overlay.lifecycle !== "open" || overlay.mode === "configuration"
            }
            onClick={() => void runOverlayCommand("set_overlay_mode", "configuration")}
          >
            Modo configuração
          </button>
          <button
            type="button"
            disabled={
              overlayPending || overlay.lifecycle !== "open" || overlay.mode === "operation"
            }
            onClick={() => void runOverlayCommand("set_overlay_mode", "operation")}
          >
            Modo operação
          </button>
          <button
            type="button"
            className="button--danger"
            disabled={overlayPending || overlay.lifecycle === "closed"}
            onClick={() => void runOverlayCommand("close_overlay")}
          >
            Fechar overlay
          </button>
        </div>
        {!overlay.supported ? (
          <p className="overlay-controls__note">
            Fail-closed: C1 habilita transparência e click-through apenas no Windows.
          </p>
        ) : (
          <p className="overlay-controls__note">
            Recuperação: use o ícone do Anchor na bandeja para voltar à configuração ou fechar.
          </p>
        )}
      </section>

      <section className="layout-grid">
        <article className="panel panel--visual">
          <div className="panel__heading">
            <h2>Movimento em tempo real</h2>
            <p>
              Marcador baseado diretamente em linearAccelerationMps2.x/y, sem
              integração física.
            </p>
          </div>

          <div className="motion-stage">
            <div className="motion-stage__axis motion-stage__axis--horizontal" />
            <div className="motion-stage__axis motion-stage__axis--vertical" />
            <div className="motion-stage__center" aria-hidden="true" />
            <div
              className="motion-stage__marker"
              style={{
                transform: `translate(calc(-50% + ${markerOffset.x}px), calc(-50% + ${markerOffset.y}px))`,
              }}
            />
          </div>
        </article>

        <article className="panel">
          <div className="panel__heading">
            <h2>Tilt processado</h2>
            <p>Saída utilizável vem de targetTilt; fora de válido permanece neutra.</p>
          </div>

          <dl className="data-grid">
            <div><dt>Estado</dt><dd>{getLiveTiltStateLabel(tiltSnapshot.validity)}</dd></div>
            <div><dt>Neutralização</dt><dd>{getNeutralReasonLabel(tiltSnapshot.neutralReason)}</dd></div>
            <div><dt>Roll utilizável</dt><dd>{formatNumber(targetTilt.rollRad, 4)}</dd></div>
            <div><dt>Pitch utilizável</dt><dd>{formatNumber(targetTilt.pitchRad, 4)}</dd></div>
            <div><dt>Idade da amostra</dt><dd>{formatAge(tiltSnapshot.lastSampleAgeMs)}</dd></div>
            <div><dt>Fingerprint</dt><dd>{shortFingerprint(tiltSnapshot.calibrationProfileFingerprint)}</dd></div>
            <div><dt>Descartes ingresso</dt><dd>{formatNumber(tiltSnapshot.integrationMetrics.ingressDroppedEvents, 0)}</dd></div>
            <div><dt>Última estimativa (diag.)</dt><dd>{tiltSnapshot.lastEstimate ? `${formatNumber(tiltSnapshot.lastEstimate.rollRad, 4)} / ${formatNumber(tiltSnapshot.lastEstimate.pitchRad, 4)}` : "--"}</dd></div>
          </dl>
        </article>

        <article className="panel">
          <div className="panel__heading">
            <h2>Amostra atual</h2>
            <p>Somente amostras aceitas pelo receptor local chegam até esta tela.</p>
          </div>

          <dl className="data-grid">
            <div>
              <dt>Accel X</dt>
              <dd>{formatSampleNumber(displayedSample, (sample) => sample.linearAccelerationMps2.x)}</dd>
            </div>
            <div>
              <dt>Accel Y</dt>
              <dd>{formatSampleNumber(displayedSample, (sample) => sample.linearAccelerationMps2.y)}</dd>
            </div>
            <div>
              <dt>Accel Z</dt>
              <dd>{formatSampleNumber(displayedSample, (sample) => sample.linearAccelerationMps2.z)}</dd>
            </div>
            <div>
              <dt>Grav X</dt>
              <dd>{formatSampleNumber(displayedSample, (sample) => sample.gravityMps2.x)}</dd>
            </div>
            <div>
              <dt>Grav Y</dt>
              <dd>{formatSampleNumber(displayedSample, (sample) => sample.gravityMps2.y)}</dd>
            </div>
            <div>
              <dt>Grav Z</dt>
              <dd>{formatSampleNumber(displayedSample, (sample) => sample.gravityMps2.z)}</dd>
            </div>
            <div>
              <dt>Gyro X</dt>
              <dd>{formatSampleNumber(displayedSample, (sample) => sample.angularVelocityRadS.x)}</dd>
            </div>
            <div>
              <dt>Gyro Y</dt>
              <dd>{formatSampleNumber(displayedSample, (sample) => sample.angularVelocityRadS.y)}</dd>
            </div>
            <div>
              <dt>Gyro Z</dt>
              <dd>{formatSampleNumber(displayedSample, (sample) => sample.angularVelocityRadS.z)}</dd>
            </div>
            <div>
              <dt>Sequência</dt>
              <dd>{formatNumber(snapshot.lastSequence, 0)}</dd>
            </div>
            <div>
              <dt>Sessão</dt>
              <dd>{formatText(snapshot.activeSessionId)}</dd>
            </div>
            <div>
              <dt>Idade</dt>
              <dd>{formatAge(snapshot.lastValidAgeMs)}</dd>
            </div>
          </dl>
        </article>

        <article className="panel">
          <div className="panel__heading">
            <h2>Estado do receptor</h2>
            <p>Transições preservam a última amostra válida para diagnóstico.</p>
          </div>

          <dl className="data-list">
            <div>
              <dt>Status</dt>
              <dd>{getStatusLabel(snapshot.status)}</dd>
            </div>
            <div>
              <dt>Remetente ativo</dt>
              <dd>{formatText(snapshot.activeSender)}</dd>
            </div>
            <div>
              <dt>Entrada</dt>
              <dd>{RECEIVER_SOURCE_LABEL}</dd>
            </div>
          </dl>
        </article>

        <article className="panel panel--metrics">
          <div className="panel__heading">
            <h2>Métricas do receptor</h2>
            <p>Contadores atualizados via snapshot de leitura.</p>
          </div>

          <dl className="data-grid">
            <div>
              <dt>Datagramas recebidos</dt>
              <dd>{formatNumber(snapshot.metrics.receivedDatagrams, 0)}</dd>
            </div>
            <div>
              <dt>Amostras aceitas</dt>
              <dd>{formatNumber(snapshot.metrics.acceptedSamples, 0)}</dd>
            </div>
            <div>
              <dt>Oversized</dt>
              <dd>{formatNumber(snapshot.metrics.oversizedDatagrams, 0)}</dd>
            </div>
            <div>
              <dt>Inválidos</dt>
              <dd>{formatNumber(snapshot.metrics.invalidPackets, 0)}</dd>
            </div>
            <div>
              <dt>Duplicados/fora de ordem</dt>
              <dd>
                {formatNumber(snapshot.metrics.duplicateOrOutOfOrderPackets, 0)}
              </dd>
            </div>
            <div>
              <dt>Sessão estrangeira</dt>
              <dd>{formatNumber(snapshot.metrics.foreignSessionPackets, 0)}</dd>
            </div>
            <div>
              <dt>Rate limited</dt>
              <dd>{formatNumber(snapshot.metrics.rateLimitedDatagrams, 0)}</dd>
            </div>
          </dl>
        </article>
      </section>
    </main>
  );
}

export default App;
