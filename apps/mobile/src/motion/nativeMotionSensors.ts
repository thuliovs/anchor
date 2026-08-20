import type { EventSubscription } from 'react-native';

import NativeMotionSensors, {
  type NativeMotionFrame,
  type SensorAvailability,
  type StartResult,
} from '../../specs/NativeMotionSensors';

export type { NativeMotionFrame, SensorAvailability, StartResult };

export interface MotionSensorsModule {
  getAvailability(): Promise<SensorAvailability>;
  start(rateHz: number): Promise<StartResult>;
  stop(): void;
  onMotionFrame(listener: (frame: NativeMotionFrame) => void): EventSubscription;
}

export function getMotionSensorsModule(): MotionSensorsModule | null {
  const module = NativeMotionSensors;
  if (!module) {
    return null;
  }

  return {
    getAvailability: () => module.getAvailability(),
    start: rateHz => module.start(rateHz),
    stop: () => module.stop(),
    onMotionFrame: listener => module.onMotionFrame(listener),
  };
}
