import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import type { MotionSampleV1 } from "@anchor/protocol";
import {
  createSequentialPoller,
  EMPTY_DIAGNOSTIC_ERRORS,
  setEventBridgeError,
  setSnapshotError,
  type DiagnosticErrorState,
} from "./diagnostics-runtime";
import {
  EMPTY_RECEIVER_SNAPSHOT,
  MOTION_SAMPLE_EVENT,
  RECEIVER_SOURCE_LABEL,
  SNAPSHOT_POLL_INTERVAL_MS,
  formatAge,
  formatNumber,
  formatSampleNumber,
  formatText,
  getDisplayedSample,
  getStatusLabel,
  mapAccelerationToOffset,
  type ReceiverSnapshotDto,
} from "./diagnostics";
import "./App.css";

const VISUAL_RADIUS_PX = 108;

function App() {
  const [snapshot, setSnapshot] =
    useState<ReceiverSnapshotDto>(EMPTY_RECEIVER_SNAPSHOT);
  const [liveSample, setLiveSample] = useState<MotionSampleV1 | null>(null);
  const [errors, setErrors] =
    useState<DiagnosticErrorState>(EMPTY_DIAGNOSTIC_ERRORS);

  useEffect(() => {
    let isDisposed = false;
    let unlisten: (() => void) | null = null;

    const poller = createSequentialPoller({
      intervalMs: SNAPSHOT_POLL_INTERVAL_MS,
      scheduler: {
        schedule: (callback, delayMs) => window.setTimeout(callback, delayMs),
        cancel: (handle) => window.clearTimeout(handle),
      },
      poll: () => invoke<ReceiverSnapshotDto>("get_receiver_snapshot"),
      onSuccess: (nextSnapshot) => {
        if (isDisposed) {
          return;
        }

        setSnapshot(nextSnapshot);
        setErrors((current) => setSnapshotError(current, null));
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

  const displayedSample = getDisplayedSample(
    snapshot.status,
    liveSample,
    snapshot.lastSample,
  );
  const markerOffset = mapAccelerationToOffset(displayedSample, VISUAL_RADIUS_PX);

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
