import NativeUdpSender from '../../specs/NativeUdpSender';

export interface UdpSenderModule {
  open(host: string, port: number): Promise<void>;
  send(payload: string): Promise<void>;
  close(): void;
  openLatency?(host: string, port: number, measurementId: string): Promise<void>;
  sendLatency?(samplePayload: string, sensorEventTimestampUs: number): Promise<void>;
  closeLatency?(): void;
}

export function getUdpSenderModule(): UdpSenderModule | null {
  const module = NativeUdpSender;
  if (!module) {
    return null;
  }

  return {
    open: (host, port) => module.open(host, port),
    send: payload => module.send(payload),
    close: () => module.close(),
    openLatency: (host, port, measurementId) => module.openLatency(host, port, measurementId),
    sendLatency: (payload, timestamp) => module.sendLatency(payload, timestamp),
    closeLatency: () => module.closeLatency(),
  };
}
