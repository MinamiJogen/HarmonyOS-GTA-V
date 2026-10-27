// Copyright (c) 2026 GTA V iOS port contributors. Original source MIT;
// see upstream/fivestar-input-8e2e4628/LICENSE.
// HarmonyOS adaptation distributed under GPL-3.0-or-later.
// Adapted from GyroInput.swift / NativePortViewController.swift at 8e2e4628.
import { FiveStarOutput, FiveStarInput } from './FiveStarInput';
import { FiveStarContext, FiveStarPoint, fiveStarDriving } from './FiveStarLayout';
import { FiveStarSettings } from './FiveStarSettings';

export interface FiveStarMotionVector { x: number; y: number; z: number; }
function clamp(value: number, low: number, high: number): number { return Math.max(low, Math.min(high, value)); }
function finiteVector(value: FiveStarMotionVector): boolean {
  return Number.isFinite(value.x) && Number.isFinite(value.y) && Number.isFinite(value.z);
}

/** Sensor-independent integrator: radians, seconds, and monotonic milliseconds. */
export class FiveStarMotionCore {
  private rotation: number = 0;
  private gravity: FiveStarMotionVector | undefined = undefined;
  private gravityAt: number = -1;
  private lastGyroTimestamp: number = -1;
  private gyroAt: number = -1;
  private accumulated: FiveStarPoint = { x: 0, y: 0 };
  private smoothed: FiveStarPoint = { x: 0, y: 0 };
  private tiltNeutral: number | undefined = undefined;
  private tiltSince: number = -1;
  private tiltSmoothed: number = 0;

  reset(): void {
    this.gravity = undefined; this.gravityAt = -1; this.lastGyroTimestamp = -1; this.gyroAt = -1;
    this.accumulated = { x: 0, y: 0 }; this.smoothed = { x: 0, y: 0 }; this.resetTilt();
  }
  private resetTilt(): void { this.tiltNeutral = undefined; this.tiltSince = -1; this.tiltSmoothed = 0; }
  setRotation(rotation: number): void {
    const next: number = ((Math.round(rotation) % 4) + 4) % 4;
    if (next !== this.rotation) { this.rotation = next; this.reset(); }
  }
  /** Clockwise display rotations map device axes into screen-right / screen-up axes. */
  private screen(x: number, y: number): FiveStarPoint {
    switch (this.rotation) {
      case 1: return { x: -y, y: x };
      case 2: return { x: -x, y: -y };
      case 3: return { x: y, y: -x };
      default: return { x: x, y: y };
    }
  }
  /** Harmony gravity is in m/s². Convert its support-acceleration vector to unit down. */
  gravitySample(value: FiveStarMotionVector, nowMs: number): void {
    if (!finiteVector(value) || !Number.isFinite(nowMs)) { return; }
    const length: number = Math.hypot(value.x, value.y, value.z);
    if (length < 0.1) { return; }
    this.gravity = { x: -value.x / length, y: -value.y / length, z: -value.z / length };
    this.gravityAt = nowMs;
  }
  gyroscopeSample(rate: FiveStarMotionVector, timestampNs: number, nowMs: number): void {
    if (!finiteVector(rate) || !Number.isFinite(timestampNs) || !Number.isFinite(nowMs)) { return; }
    const previous: number = this.lastGyroTimestamp;
    this.lastGyroTimestamp = timestampNs; this.gyroAt = nowMs;
    if (previous < 0 || timestampNs <= previous || !this.gravity || nowMs - this.gravityAt > 250) { return; }
    const dt: number = clamp((timestampNs - previous) / 1000000000, 0, 0.05);
    const yaw: number = rate.x * this.gravity.x + rate.y * this.gravity.y + rate.z * this.gravity.z;
    const pitch: number = -this.screen(rate.x, rate.y).x;
    this.accumulated.x += yaw * dt; this.accumulated.y += pitch * dt;
  }
  rate(active: boolean, elapsed: number, nowMs: number): FiveStarPoint {
    const turned: FiveStarPoint = this.accumulated;
    this.accumulated = { x: 0, y: 0 };
    if (!active || elapsed <= 0 || this.gyroAt < 0 || nowMs - this.gyroAt > 250 || nowMs - this.gravityAt > 250) {
      this.smoothed = { x: 0, y: 0 }; return { x: 0, y: 0 };
    }
    const raw: FiveStarPoint = { x: turned.x / elapsed, y: turned.y / elapsed };
    const blend: number = clamp((Math.hypot(raw.x, raw.y) - 0.035) / (0.14 - 0.035), 0, 1);
    this.smoothed.x = this.smoothed.x * 0.5 + raw.x * 0.5;
    this.smoothed.y = this.smoothed.y * 0.5 + raw.y * 0.5;
    const tighten = (value: number): number => Math.abs(value) < 0.026 ? value * Math.abs(value) / 0.026 : value;
    return { x: tighten(raw.x * blend + this.smoothed.x * (1 - blend)),
      y: tighten(raw.y * blend + this.smoothed.y * (1 - blend)) };
  }
  steeringAngle(nowMs: number): number | undefined {
    if (!this.gravity || nowMs - this.gravityAt > 250) { return undefined; }
    const screen: FiveStarPoint = this.screen(this.gravity.x, this.gravity.y);
    if (Math.hypot(screen.x, screen.y) <= 0.3) { return undefined; }
    return Math.atan2(screen.x, -screen.y);
  }
  apply(output: FiveStarOutput, context: FiveStarContext, aiming: boolean, settings: FiveStarSettings,
    elapsed: number, active: boolean, nowMs: number, flyingWithRightStick: boolean = false): FiveStarOutput {
    if (!active) { this.reset(); return output; }
    // Immediate touch samples have zero elapsed; preserve sensor travel for the next frame.
    if (elapsed <= 0) { return output; }
    const dt: number = Math.min(elapsed, 0.05);
    const driving: boolean = fiveStarDriving(context);
    const tiltWanted: boolean = driving && settings.tiltSteer !== 0;
    if (tiltWanted) {
      const angle: number | undefined = this.steeringAngle(nowMs);
      if (angle !== undefined) {
        if (this.tiltSince < 0) { this.tiltSince = nowMs; }
        if (this.tiltNeutral === undefined && nowMs - this.tiltSince > 150) { this.tiltNeutral = angle; }
        if (this.tiltNeutral !== undefined) {
          const difference: number = Math.atan2(Math.sin(angle - this.tiltNeutral), Math.cos(angle - this.tiltNeutral));
          let degrees: number = difference * 180 / Math.PI;
          degrees = Math.abs(degrees) < 1.5 ? 0 : degrees - Math.sign(degrees) * 1.5;
          const raw: number = clamp(degrees / settings.tiltFullLock, -1, 1);
          const curved: number = Math.sign(raw) * Math.pow(Math.abs(raw), 1.4);
          this.tiltSmoothed += (curved - this.tiltSmoothed) * Math.min(1, dt / 0.06);
          const fade: number = clamp((nowMs - this.tiltSince - 150) / 300, 0, 1);
          const tilt: number = this.tiltSmoothed * fade;
          if (settings.tiltSteer === 2 && Math.abs(output.leftX) < 0.05) { output.leftX = tilt; }
          else if (settings.tiltSteer === 1) { output.leftX = clamp(output.leftX + tilt * 0.6, -1, 1); }
        }
      }
    } else { this.resetTilt(); }
    const gyroWanted: boolean = settings.gyroAim !== 0 && !flyingWithRightStick && context !== 'menu';
    const gyroActive: boolean = gyroWanted && (aiming || output.leftTrigger > 0.1 || (settings.gyroAim === 2 && context === 'onFoot'));
    const rate: FiveStarPoint = this.rate(gyroActive, elapsed, nowMs);
    if (gyroActive) {
      const gain: number = 350 * settings.gyroSensitivity;
      const invert: number = settings.gyroInvertY ? -1 : 1;
      output.rightX = clamp(output.rightX + FiveStarInput.lookAxis(rate.x * gain), -1, 1);
      output.rightY = clamp(output.rightY - FiveStarInput.lookAxis(rate.y * gain * invert), -1, 1);
    }
    return output;
  }
}
