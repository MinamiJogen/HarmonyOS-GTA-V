import { TouchFrame, touchSlotFrame } from './TouchGeometry';
import { TOUCH_SLOTS, TouchSlot } from './TouchLayout';

export interface TouchSlotAdjustment { x: number; y: number; diameter: number; }
export interface TouchFamilySettings {
  opacity: number;
  sensitivity: number;
  slots: Record<string, TouchSlotAdjustment>;
}
export interface TouchLayoutSettings { version: number; families: Record<string, TouchFamilySettings>; }
export function defaultTouchFamilySettings(): TouchFamilySettings { return { opacity: 55, sensitivity: 1, slots: {} }; }
export function touchSettingsFamily(profile: string, width: number, height: number): string {
  const family: string = profile.indexOf('FOOT_') === 0 ? 'FOOT' : profile;
  return `${family}:${width / Math.max(1, height) < 1.5 ? 'fold' : 'wide'}`;
}
export function adjustedTouchFrame(slot: string, width: number, height: number, settings: TouchFamilySettings): TouchFrame {
  const frame: TouchFrame = touchSlotFrame(slot, width, height);
  const adjustment: TouchSlotAdjustment | undefined = settings.slots[slot];
  if (!adjustment) { return frame; }
  return { x: frame.x + adjustment.x, y: frame.y + adjustment.y, diameter: adjustment.diameter };
}
/** Validate all reserved slots, including currently hidden emergency actions, before saving a user's edit. */
export function validateTouchSettings(width: number, height: number, settings: TouchFamilySettings): string {
  if (width < 640 || height < 360) { return '当前紧凑屏幕规格尚未验收，暂不保存自定义位置。'; }
  const frames: TouchFrame[] = [];
  const slotIds: string[] = TOUCH_SLOTS.map((slot: TouchSlot): string => slot.id).concat(['RUN']);
  for (let i = 0; i < slotIds.length; i++) {
    const slot: string = slotIds[i];
    const f: TouchFrame = adjustedTouchFrame(slot, width, height, settings);
    if (!Number.isFinite(f.x) || !Number.isFinite(f.y) || !Number.isFinite(f.diameter) || f.diameter < 44 || f.diameter > 104) { return '按键直径需为 44–104 vp。'; }
    const radius: number = f.diameter / 2;
    if (f.x - radius < 8 || f.y - radius < 8 || f.x + radius > width - 8 || f.y + radius > height - 8) { return '按键需要完整留在游戏安全区内。'; }
    if (f.x - radius < width * 0.22 + 12 && f.y - radius < height * 0.3 + 12) { return '此位置会遮挡左上小地图与玩家状态区。'; }
    const joyDistance: number = Math.sqrt((f.x - 96) * (f.x - 96) + (f.y - (height - 96)) * (f.y - (height - 96)));
    if (joyDistance < 64 + radius + 8) { return '此位置与左下摇杆触摸区重叠。'; }
    for (let j = 0; j < frames.length; j++) {
      const old: TouchFrame = frames[j];
      const distance: number = Math.sqrt((f.x - old.x) * (f.x - old.x) + (f.y - old.y) * (f.y - old.y));
      if (distance < radius + old.diameter / 2 + 8) { return '此位置与其它按键或预留任务槽重叠。'; }
    }
    frames.push(f);
  }
  return '';
}
export function sanitizeTouchFamily(value: TouchFamilySettings | undefined): TouchFamilySettings {
  if (!value) { return defaultTouchFamilySettings(); }
  return { opacity: Number.isFinite(value.opacity) ? Math.max(35, Math.min(70, value.opacity)) : 55,
    sensitivity: Number.isFinite(value.sensitivity) ? Math.max(0.3, Math.min(2, value.sensitivity)) : 1,
    slots: value.slots || {} };
}
