// Adapted from Five Star Port 8e2e46281159d09005fea4748b5b9926e919fc0a.
// Copyright (c) 2026 GTA V iOS port contributors. MIT; see upstream/fivestar-input-8e2e4628/LICENSE.
export interface FiveStarLayoutEntry { x: number; y: number; scale: number; hidden: boolean; }
export interface FiveStarSettings {
  version: number;
  controlOpacity: number;
  controlScale: number;
  cameraTouchMode: number;
  preferredSwipeMode: number;
  swipeLookSensitivity: number;
  swipeAimSensitivity: number;
  mouseLookSensitivity: number;
  mouseAimSensitivity: number;
  lookSensitivity: number;
  aimSensitivity: number;
  fireAlsoAims: boolean;
  targetingMode: number;
  sprintButton: boolean;
  steeringButtons: boolean;
  airplaneRightStick: boolean;
  hideTouchWithController: boolean;
  gyroAim: number;
  gyroSensitivity: number;
  gyroInvertY: boolean;
  tiltSteer: number;
  tiltFullLock: number;
  layoutEntries: Record<string, FiveStarLayoutEntry>;
}
export function defaultFiveStarSettings(): FiveStarSettings {
  return { version: 1, controlOpacity: 0.8, controlScale: 1, cameraTouchMode: 0, preferredSwipeMode: 2,
    swipeLookSensitivity: 3, swipeAimSensitivity: 3, mouseLookSensitivity: 5, mouseAimSensitivity: 5,
    lookSensitivity: 2, aimSensitivity: 1.3, fireAlsoAims: true, targetingMode: 1,
    sprintButton: false, steeringButtons: false, airplaneRightStick: false, hideTouchWithController: true,
    gyroAim: 0, gyroSensitivity: 1, gyroInvertY: false, tiltSteer: 0, tiltFullLock: 22, layoutEntries: {} };
}
function range(value: number, fallback: number, low: number, high: number): number {
  return Number.isFinite(value) ? Math.max(low, Math.min(high, value)) : fallback;
}
export function sanitizeFiveStarSettings(value?: FiveStarSettings): FiveStarSettings {
  const d: FiveStarSettings = defaultFiveStarSettings();
  if (!value) { return d; }
  const entries: Record<string, FiveStarLayoutEntry> = {};
  if (value.layoutEntries) {
    for (const key of Object.keys(value.layoutEntries)) {
      const entry: FiveStarLayoutEntry = value.layoutEntries[key];
      if (!entry || !Number.isFinite(entry.x) || !Number.isFinite(entry.y)) { continue; }
      entries[key] = { x: range(entry.x, 0.5, 0, 1), y: range(entry.y, 0.5, 0, 1),
        scale: range(entry.scale, 1, 0.6, 1.8), hidden: entry.hidden === true };
    }
  }
  return { version: 1, controlOpacity: range(value.controlOpacity, 0.8, 0.2, 1),
    controlScale: range(value.controlScale, 1, 0.7, 1.5),
    // The pinned upstream getter maps either saved swipe variant to native mouse.
    cameraTouchMode: value.cameraTouchMode === 0 || value.cameraTouchMode === undefined ? 0 : 2,
    preferredSwipeMode: value.preferredSwipeMode === 1 ? 1 : 2,
    swipeLookSensitivity: range(value.swipeLookSensitivity, 3, 0.2, 15),
    swipeAimSensitivity: range(value.swipeAimSensitivity, 3, 0.2, 15),
    mouseLookSensitivity: range(value.mouseLookSensitivity, 5, 0.2, 15),
    mouseAimSensitivity: range(value.mouseAimSensitivity, 5, 0.2, 15),
    lookSensitivity: range(value.lookSensitivity, 2, 0.3, 3), aimSensitivity: range(value.aimSensitivity, 1.3, 0.3, 4),
    fireAlsoAims: value.fireAlsoAims !== false, targetingMode: range(value.targetingMode, 1, 0, 3),
    sprintButton: value.sprintButton === true, steeringButtons: value.steeringButtons === true,
    airplaneRightStick: value.airplaneRightStick === true, hideTouchWithController: value.hideTouchWithController !== false,
    gyroAim: range(value.gyroAim, 0, 0, 2), gyroSensitivity: range(value.gyroSensitivity, 1, 0, 10),
    gyroInvertY: value.gyroInvertY === true, tiltSteer: range(value.tiltSteer, 0, 0, 2),
    tiltFullLock: range(value.tiltFullLock, 22, 10, 45), layoutEntries: entries };
}
