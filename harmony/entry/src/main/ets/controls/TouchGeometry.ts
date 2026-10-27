import { TOUCH_SLOTS, TouchSlot } from './TouchLayout';
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
/** Four visible 44vp keys, with no surrounding invisible movement target. */
export function directionButtonFrame(direction: string, width: number, height: number): TouchFrame {
  const x: number = direction === 'left' ? 44 : (direction === 'right' ? 148 : 96);
  const bottom: number = direction === 'up' ? 136 : (direction === 'down' ? 32 : 84);
  return { x: Math.min(Math.max(22, x), Math.max(22, width - 22)),
    y: Math.max(22, height - bottom), diameter: 44 };
}
