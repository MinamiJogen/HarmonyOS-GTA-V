import { sensor } from '@kit.SensorServiceKit';
import { display } from '@kit.ArkUI';
import { systemDateTime } from '@kit.BasicServicesKit';
import { FiveStarOutput } from './FiveStarInput';
import { FiveStarContext, fiveStarDriving } from './FiveStarLayout';
import { FiveStarSettings } from './FiveStarSettings';
import { FiveStarMotionCore } from './FiveStarMotionMath';

// Copyright (c) 2026 GTA V iOS port contributors. Original source MIT;
// see upstream/fivestar-input-8e2e4628/LICENSE.
// HarmonyOS adaptation distributed under GPL-3.0-or-later.
// SDK: sensor timestamp and interval are ns; gyroscope rates are rad/s.
export class FiveStarMotion {
  private core: FiveStarMotionCore = new FiveStarMotionCore();
  private running: boolean = false;
  private gravitySubscribed: boolean = false;
  private gyroSubscribed: boolean = false;
  private displaySubscribed: boolean = false;
  private subscriptionEpoch: number = 0;
  private lastContext: FiveStarContext = 'onFoot';
  private gyroCallback: (data: sensor.GyroscopeResponse) => void = (data: sensor.GyroscopeResponse): void => {
    if (this.running) { this.core.gyroscopeSample(data, data.timestamp, this.now()); }
  };
  private gravityCallback: (data: sensor.GravityResponse) => void = (data: sensor.GravityResponse): void => {
    if (this.running) { this.core.gravitySample(data, this.now()); }
  };
  private displayCallback: (id: number) => void = (_id: number): void => { if (this.running) { this.readRotation(); } };
  private now(): number { return systemDateTime.getUptime(systemDateTime.TimeType.STARTUP, false); }
  private readRotation(): void {
    try { this.core.setRotation(display.getDefaultDisplaySync().rotation); } catch (_) {}
  }
  setRunning(wanted: boolean): void {
    if (!wanted) { this.stop(); return; }
    if (this.running) { return; }
    this.running = true; this.readRotation();
    const epoch: number = ++this.subscriptionEpoch;
    this.gyroCallback = (data: sensor.GyroscopeResponse): void => {
      if (this.running && this.subscriptionEpoch === epoch) { this.core.gyroscopeSample(data, data.timestamp, this.now()); }
    };
    this.gravityCallback = (data: sensor.GravityResponse): void => {
      if (this.running && this.subscriptionEpoch === epoch) { this.core.gravitySample(data, this.now()); }
    };
    this.displayCallback = (_id: number): void => { if (this.running && this.subscriptionEpoch === epoch) { this.readRotation(); } };
    try { sensor.on(sensor.SensorId.GRAVITY, this.gravityCallback, { interval: 8333333 }); this.gravitySubscribed = true; } catch (_) {}
    try { sensor.on(sensor.SensorId.GYROSCOPE, this.gyroCallback, { interval: 8333333 }); this.gyroSubscribed = true; } catch (_) {}
    try { display.on('change', this.displayCallback); this.displaySubscribed = true; } catch (_) {}
  }
  stop(): void {
    this.running = false;
    this.subscriptionEpoch++;
    if (this.gyroSubscribed) { try { sensor.off(sensor.SensorId.GYROSCOPE, this.gyroCallback); } catch (_) {} this.gyroSubscribed = false; }
    if (this.gravitySubscribed) { try { sensor.off(sensor.SensorId.GRAVITY, this.gravityCallback); } catch (_) {} this.gravitySubscribed = false; }
    if (this.displaySubscribed) { try { display.off('change', this.displayCallback); } catch (_) {} this.displaySubscribed = false; }
    this.core.reset();
  }
  apply(output: FiveStarOutput, context: FiveStarContext, aiming: boolean, settings: FiveStarSettings,
    elapsed: number, active: boolean, flyingWithRightStick: boolean = false): FiveStarOutput {
    const tiltWanted: boolean = active && fiveStarDriving(context) && settings.tiltSteer !== 0;
    const gyroWanted: boolean = active && settings.gyroAim !== 0 && !flyingWithRightStick && context !== 'menu';
    // Context transitions clear accumulated motion and the steering neutral before restarting.
    if (context !== this.lastContext) { this.stop(); this.lastContext = context; }
    this.setRunning(tiltWanted || gyroWanted);
    return this.core.apply(output, context, aiming, settings, elapsed, active, this.now(), flyingWithRightStick);
  }
}
