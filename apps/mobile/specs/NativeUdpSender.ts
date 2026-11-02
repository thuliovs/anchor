import type { TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export interface Spec extends TurboModule {
  open(host: string, port: number): Promise<void>;
  send(payload: string): Promise<void>;
  close(): void;
  openLatency(host: string, port: number, measurementId: string): Promise<void>;
  sendLatency(samplePayload: string, sensorEventTimestampUs: number): Promise<void>;
  closeLatency(): void;
}

export default TurboModuleRegistry.get<Spec>('NativeUdpSender');
