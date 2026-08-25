import type { UdpSenderModule } from './nativeUdpSender';
import type { UdpDestination } from './udpDestination';

export interface SequentialUdpSenderMetrics {
  offeredDatagrams: number;
  sentDatagrams: number;
  droppedBackpressure: number;
  rejectedPayloads: number;
  sendErrors: number;
  lastTransportError: string | null;
}

export type SequentialUdpSenderState = 'closed' | 'opening' | 'open' | 'error';

export interface SequentialUdpSenderSnapshot {
  state: SequentialUdpSenderState;
  destination: UdpDestination | null;
  metrics: SequentialUdpSenderMetrics;
}

interface SequentialUdpSenderOptions {
  transport: UdpSenderModule;
  onTransportError?: (error: Error) => void;
}

const EMPTY_METRICS: SequentialUdpSenderMetrics = {
  offeredDatagrams: 0,
  sentDatagrams: 0,
  droppedBackpressure: 0,
  rejectedPayloads: 0,
  sendErrors: 0,
  lastTransportError: null,
};

export class SequentialUdpSender {
  private readonly transport: UdpSenderModule;
  private readonly onTransportError?: (error: Error) => void;

  private generation = 0;
  private state: SequentialUdpSenderState = 'closed';
  private destination: UdpDestination | null = null;
  private inFlight = false;
  private pendingLatest: string | null = null;
  private metrics: SequentialUdpSenderMetrics = { ...EMPTY_METRICS };

  constructor(options: SequentialUdpSenderOptions) {
    this.transport = options.transport;
    this.onTransportError = options.onTransportError;
  }

  async open(destination: UdpDestination): Promise<void> {
    const generation = this.nextGeneration();

    if (this.state !== 'closed' || this.destination !== null) {
      this.transport.close();
    }
    this.state = 'opening';
    this.destination = destination;
    this.inFlight = false;
    this.pendingLatest = null;
    this.metrics = { ...EMPTY_METRICS };

    try {
      await this.transport.open(destination.host, destination.port);
    } catch (error) {
      if (this.isCurrentGeneration(generation)) {
        this.state = 'error';
        this.metrics.lastTransportError = toMessage(error);
        this.transport.close();
      }
      throw error;
    }

    if (!this.isCurrentGeneration(generation)) {
      return;
    }

    this.state = 'open';
  }

  offerDatagram(payload: string): void {
    this.metrics.offeredDatagrams += 1;

    if (this.state !== 'open' || this.destination === null) {
      this.metrics.rejectedPayloads += 1;
      return;
    }

    if (this.inFlight) {
      if (this.pendingLatest !== null) {
        this.metrics.droppedBackpressure += 1;
      }
      this.pendingLatest = payload;
      return;
    }

    this.dispatch(payload, this.generation);
  }

  recordRejectedPayload(error: unknown): void {
    this.metrics.rejectedPayloads += 1;
    this.metrics.lastTransportError = toMessage(error);
  }

  close(): void {
    this.nextGeneration();
    this.state = 'closed';
    this.destination = null;
    this.inFlight = false;
    this.pendingLatest = null;
    this.transport.close();
  }

  getMetrics(): SequentialUdpSenderMetrics {
    return { ...this.metrics };
  }

  getSnapshot(): SequentialUdpSenderSnapshot {
    return {
      state: this.state,
      destination: this.destination,
      metrics: this.getMetrics(),
    };
  }

  private dispatch(payload: string, generation: number): void {
    this.inFlight = true;

    this.transport.send(payload).then(() => {
      if (!this.isCurrentGeneration(generation) || this.state !== 'open') {
        return;
      }

      this.metrics.sentDatagrams += 1;
      this.inFlight = false;

      if (this.pendingLatest !== null) {
        const nextPayload = this.pendingLatest;
        this.pendingLatest = null;
        this.dispatch(nextPayload, generation);
      }
    }).catch(error => {
      if (!this.isCurrentGeneration(generation) || this.state === 'closed') {
        return;
      }

      this.state = 'error';
      this.inFlight = false;
      this.pendingLatest = null;
      this.metrics.sendErrors += 1;
      this.metrics.lastTransportError = toMessage(error);
      this.onTransportError?.(toError(error));
    });
  }

  private nextGeneration(): number {
    this.generation += 1;
    return this.generation;
  }

  private isCurrentGeneration(generation: number): boolean {
    return this.generation === generation;
  }
}

function toMessage(error: unknown): string {
  return error instanceof Error && error.message ? error.message : String(error);
}

function toError(error: unknown): Error {
  return error instanceof Error ? error : new Error(toMessage(error));
}
