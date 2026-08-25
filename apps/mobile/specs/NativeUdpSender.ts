import type { TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export interface Spec extends TurboModule {
  open(host: string, port: number): Promise<void>;
  send(payload: string): Promise<void>;
  close(): void;
}

export default TurboModuleRegistry.get<Spec>('NativeUdpSender');
