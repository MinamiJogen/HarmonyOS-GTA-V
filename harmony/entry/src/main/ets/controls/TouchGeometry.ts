import { TOUCH_SLOTS, TouchSlot } from './TouchLayout';
import { clampTouch, TouchVector } from './TouchInput';
export interface TouchFrame { x: number; y: number; diameter: number; }
export function touchSlotFrame(id: string, width: number, height: number): TouchFrame {
  if (id === 'RUN') { return { x: 200, y: height - 32, diameter: 44 }; }
  const compact: boolean = width < 640 || height < 360;
  if (compact) {
    let x: number = width - 48; let y: number = height - 58; let d: number = 44;
    switch (id) {
      case 'P0': d = 60; break;
      case 'P1': x = width - 118; y = height - 56; d = 52; break;
      case 'P2': y = height - 130; d = 52; break;
      case 'S0': x = width - 188; y = height - 56; break;
      case 'S1': x = width - 188; y = height - 120; break;
      case 'S2': x = width - 188; y = height - 184; break;
      case 'S3': x = width - 118; y = height - 130; break;
      case 'S4': x = 52; y = height - 184; break;
      case 'S5': x = 112; y = height - 184; break;
      case 'T0': x = 180; y = height - 52; break;
      case 'T1': x = 240; y = height - 52; break;
      case 'T2': x = 180; y = height - 184; break;
      case 'T3': x = 240; y = height - 184; break;
    }
    return { x: Math.max(22, x), y: Math.max(22, y), diameter: d };
  }
  for (let i = 0; i < TOUCH_SLOTS.length; i++) {
    const slot: TouchSlot = TOUCH_SLOTS[i];
    if (slot.id === id) { return { x: slot.left ? slot.dx : width - slot.dx, y: height - slot.dy, diameter: slot.diameter }; }
  }
  return { x: width - 168, y: height - 268, diameter: 56 };
}
// Keep only the most recent geometry; no per-gesture or unbounded viewport cache.
let joystickFallbackKey: string = '';
let joystickFallbackCentres: TouchVector[] = [];

/** Project the floating ring to a legal center, checking the entire ring against HUD and buttons. */
export function legalJoystickOrigin(x: number, y: number, width: number, height: number, slots: string[], customFrames: TouchFrame[] = [], prewarm: boolean = false): TouchVector {
  const minimumY: number = Math.max(64, height * 0.3 + 76);
  const maximumX: number = Math.max(64, width * 0.35 - 8);
  const frames: TouchFrame[] = slots.map((slot: string, index: number): TouchFrame => index < customFrames.length ? customFrames[index] : touchSlotFrame(slot, width, height));
  const legal = (px: number, py: number): boolean => {
    if (px < 72 || px > maximumX || py < minimumY || py > height - 72) { return false; }
    for (let i = 0; i < frames.length; i++) {
      const f: TouchFrame = frames[i];
      const dx: number = px - f.x; const dy: number = py - f.y;
      const exclusion: number = 64 + f.diameter / 2 + 8;
      if (dx * dx + dy * dy < exclusion * exclusion) { return false; }
    }
    return true;
  };
  // Common Down in an empty area is synchronous and never enters the grid search.
  const requested: TouchVector = { x: clampTouch(x, 72, maximumX), y: clampTouch(y, minimumY, Math.max(minimumY, height - 72)) };
  if (!prewarm && legal(requested.x, requested.y)) { return requested; }
  const key: string = `${width}:${height}:` + frames.map((f: TouchFrame): string => `${f.x},${f.y},${f.diameter}`).join(';');
  if (key !== joystickFallbackKey) {
    joystickFallbackKey = key; joystickFallbackCentres = [];
    const area: number = Math.max(0, maximumX - 72) * Math.max(0, height - 72 - minimumY);
    const step: number = Math.max(4, Math.ceil(Math.sqrt(area / 8192)));
    for (let py: number = minimumY; py <= height - 72; py += step) {
      for (let px: number = 72; px <= maximumX; px += step) {
        if (legal(px, py)) { joystickFallbackCentres.push({ x: px, y: py }); }
      }
    }
    if (legal(96, height - 96)) { joystickFallbackCentres.push({ x: 96, y: height - 96 }); }
  }
  if (prewarm) { return requested; }
  let best: TouchVector = { x: 96, y: height - 96 };
  let distance: number = Number.POSITIVE_INFINITY;
  for (let i = 0; i < joystickFallbackCentres.length; i++) {
    const centre: TouchVector = joystickFallbackCentres[i];
    const candidate: number = (centre.x - x) * (centre.x - x) + (centre.y - y) * (centre.y - y);
    if (candidate < distance) { distance = candidate; best = centre; }
  }
  return { x: clampTouch(best.x, 64, Math.max(64, width - 64)), y: clampTouch(best.y, 64, Math.max(64, height - 64)) };
}

/** Called on geometry/layout changes, never from the gesture recognition path. */
export function prewarmJoystickGeometry(width: number, height: number, slots: string[], frames: TouchFrame[]): void {
  legalJoystickOrigin(96, height - 96, width, height, slots, frames, true);
}
