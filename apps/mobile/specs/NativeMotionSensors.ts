import type { TurboModule, CodegenTypes } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export type Vector3 = {
  x: number;
  y: number;
  z: number;
};

export type SensorAvailability = {
  linearAcceleration: boolean;
  gravity: boolean;
  gyroscope: boolean;
};

export type StartResult = {
  sessionId: string;
  requestedRateHz: number;
};

export type NativeMotionFrame = {
  sessionId: string;
  sequence: number;
  sessionElapsedUs: number;
  linearAccelerationMps2: Vector3;
  gravityMps2: Vector3;
  angularVelocityRadS: Vector3;
};

export interface Spec extends TurboModule {
  getAvailability(): Promise<SensorAvailability>;
  start(rateHz: number): Promise<StartResult>;
  stop(): void;
  readonly onMotionFrame: CodegenTypes.EventEmitter<NativeMotionFrame>;
}

export default TurboModuleRegistry.get<Spec>('NativeMotionSensors');
