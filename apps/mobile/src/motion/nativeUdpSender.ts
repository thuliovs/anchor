import NativeUdpSender from '../../specs/NativeUdpSender';

export interface UdpSenderModule {
  open(host: string, port: number): Promise<void>;
  send(payload: string): Promise<void>;
  close(): void;
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
  };
}
