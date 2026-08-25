import {
  SequentialUdpSender,
} from '../src/motion/SequentialUdpSender';
import type { UdpSenderModule } from '../src/motion/nativeUdpSender';

type Deferred<T> = {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason?: unknown) => void;
};

function createDeferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });

  return { promise, resolve, reject };
}

async function flushMicrotasks(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

class FakeUdpSenderModule implements UdpSenderModule {
  public readonly openCalls: Array<{ host: string; port: number }> = [];
  public readonly sendCalls: string[] = [];
  public closeCallCount = 0;
  public openDeferred: Deferred<void> | null = null;
  public readonly openDeferreds: Array<Deferred<void>> = [];
  public sendDeferreds: Array<Deferred<void>> = [];
  public openMode: 'immediate' | 'deferred' = 'immediate';
  public sendMode: 'immediate' | 'deferred' = 'immediate';
  public sendError: Error | null = null;
  public isClosed = true;

  async open(host: string, port: number): Promise<void> {
    this.openCalls.push({ host, port });
    this.isClosed = false;

    if (this.openMode === 'deferred') {
      this.openDeferred = createDeferred<void>();
      this.openDeferreds.push(this.openDeferred);
      return this.openDeferred.promise;
    }
  }

  async send(payload: string): Promise<void> {
    if (this.isClosed) {
      throw new Error('transport closed');
    }

    this.sendCalls.push(payload);

    if (this.sendError) {
      throw this.sendError;
    }

    if (this.sendMode === 'deferred') {
      const deferred = createDeferred<void>();
      this.sendDeferreds.push(deferred);
      return deferred.promise;
    }
  }

  close(): void {
    this.closeCallCount += 1;
    this.isClosed = true;
  }
}

describe('SequentialUdpSender', () => {
  it('nunca sobrepoe envios pendentes', async () => {
    const transport = new FakeUdpSenderModule();
    transport.sendMode = 'deferred';
    const sender = new SequentialUdpSender({ transport });

    await sender.open({ host: '192.168.0.20', port: 57421 });
    sender.offerDatagram('sample-1');
    sender.offerDatagram('sample-2');

    await flushMicrotasks();

    expect(transport.sendCalls).toEqual(['sample-1']);
    expect(sender.getMetrics().offeredDatagrams).toBe(2);

    transport.sendDeferreds[0].resolve();
    await flushMicrotasks();

    expect(transport.sendCalls).toEqual(['sample-1', 'sample-2']);
  });

  it('faz coalescencia latest-wins enquanto um envio esta pendente', async () => {
    const transport = new FakeUdpSenderModule();
    transport.sendMode = 'deferred';
    const sender = new SequentialUdpSender({ transport });

    await sender.open({ host: '192.168.0.20', port: 57421 });
    sender.offerDatagram('sample-1');
    sender.offerDatagram('sample-2');
    sender.offerDatagram('sample-3');

    await flushMicrotasks();
    expect(transport.sendCalls).toEqual(['sample-1']);

    transport.sendDeferreds[0].resolve();
    await flushMicrotasks();

    expect(transport.sendCalls).toEqual(['sample-1', 'sample-3']);
  });

  it('contabiliza descartes por backpressure quando a pendencia e substituida', async () => {
    const transport = new FakeUdpSenderModule();
    transport.sendMode = 'deferred';
    const sender = new SequentialUdpSender({ transport });

    await sender.open({ host: '192.168.0.20', port: 57421 });
    sender.offerDatagram('sample-1');
    sender.offerDatagram('sample-2');
    sender.offerDatagram('sample-3');
    sender.offerDatagram('sample-4');

    await flushMicrotasks();

    expect(sender.getMetrics().droppedBackpressure).toBe(2);
  });

  it('close descarta a pendencia e ignora resolucao tardia', async () => {
    const transport = new FakeUdpSenderModule();
    transport.sendMode = 'deferred';
    const sender = new SequentialUdpSender({ transport });

    await sender.open({ host: '192.168.0.20', port: 57421 });
    sender.offerDatagram('sample-1');
    sender.offerDatagram('sample-2');
    await flushMicrotasks();

    sender.close();
    transport.sendDeferreds[0].resolve();
    await flushMicrotasks();

    expect(transport.closeCallCount).toBe(1);
    expect(transport.sendCalls).toEqual(['sample-1']);
    expect(sender.getMetrics().sentDatagrams).toBe(0);
  });

  it('resolve tardio de sessao antiga nao reativa sessao encerrada', async () => {
    const transport = new FakeUdpSenderModule();
    transport.openMode = 'deferred';
    const sender = new SequentialUdpSender({ transport });

    const firstOpen = sender.open({ host: '192.168.0.20', port: 57421 });
    const firstDeferred = transport.openDeferred!;
    sender.close();

    transport.openMode = 'immediate';
    await sender.open({ host: '192.168.0.30', port: 57421 });
    firstDeferred.resolve();
    await firstOpen;

    sender.offerDatagram('sample-new');
    await flushMicrotasks();

    expect(transport.openCalls).toEqual([
      { host: '192.168.0.20', port: 57421 },
      { host: '192.168.0.30', port: 57421 },
    ]);
    expect(transport.sendCalls).toEqual(['sample-new']);
  });

  it('open tardio da sessao A nao fecha o transporte ja reaberto pela sessao B', async () => {
    const transport = new FakeUdpSenderModule();
    transport.openMode = 'deferred';
    const sender = new SequentialUdpSender({ transport });

    const openA = sender.open({ host: '192.168.0.20', port: 57421 });
    const deferredA = transport.openDeferreds[0];

    sender.close();

    transport.openMode = 'immediate';
    await sender.open({ host: '192.168.0.21', port: 57421 });
    expect(transport.isClosed).toBe(false);

    deferredA.resolve();
    await openA;

    expect(transport.isClosed).toBe(false);

    sender.offerDatagram('sample-b');
    await flushMicrotasks();

    expect(transport.sendCalls).toEqual(['sample-b']);
    expect(transport.closeCallCount).toBe(1);
  });

  it('rejeicao tardia nao envia rajada nem reabre transporte', async () => {
    const transport = new FakeUdpSenderModule();
    transport.sendMode = 'deferred';
    const sender = new SequentialUdpSender({ transport });

    await sender.open({ host: '192.168.0.20', port: 57421 });
    sender.offerDatagram('sample-1');
    sender.offerDatagram('sample-2');
    await flushMicrotasks();

    sender.close();
    transport.sendDeferreds[0].reject(new Error('late send failure'));
    await flushMicrotasks();

    expect(sender.getMetrics().sendErrors).toBe(0);
    expect(transport.openCalls).toHaveLength(1);
    expect(transport.closeCallCount).toBe(1);
  });
});
