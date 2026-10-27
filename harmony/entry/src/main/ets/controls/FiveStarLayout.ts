// Adapted from Five Star Port 8e2e46281159d09005fea4748b5b9926e919fc0a.
// Copyright (c) 2026 GTA V iOS port contributors. MIT; see upstream/fivestar-input-8e2e4628/LICENSE.
import { FiveStarSettings, FiveStarLayoutEntry } from './FiveStarSettings';
export interface FiveStarPoint { x: number; y: number; }
export interface FiveStarRect { x: number; y: number; width: number; height: number; }
export interface FiveStarInsets { left: number; right: number; top: number; bottom: number; }
export type FiveStarContext = 'onFoot' | 'car' | 'bike' | 'bicycle' | 'boat' | 'sub' | 'heli' | 'plane' | 'menu';
export interface FiveStarButton {
  name: string; icon: string; label: string; frame: FiveStarRect; pressed: boolean; active: boolean;
  progress: number; pedal: boolean; slideTarget: boolean; hidden: boolean; opacity: number;
}
export interface FiveStarStickDisplay {
  id: string; origin: FiveStarPoint; knob: FiveStarPoint; baseDiameter: number;
  name: string; x: number; y: number; valueX: number; valueY: number; diameter: number; travel: number;
  visible: boolean; engaged: boolean; sprinting: boolean; opacity: number;
}
export const FIVE_STAR_ESSENTIAL: string[] = ['fire', 'aim', 'enter', 'gas', 'brake', 'pause', 'settings', 'airFire'];
export function fiveStarContext(vehicle: number, pauseMenu: boolean): FiveStarContext {
  if (pauseMenu) { return 'menu'; }
  switch (vehicle) {
    case 0: return 'onFoot'; case 2: return 'plane'; case 9: case 10: case 11: return 'heli';
    case 12: return 'bike'; case 13: return 'bicycle'; case 14: return 'boat'; case 16: return 'sub'; default: return 'car';
  }
}
export function fiveStarDriving(context: FiveStarContext): boolean {
  return ['car', 'bike', 'bicycle', 'boat', 'sub'].indexOf(context) >= 0;
}
export function fiveStarFlying(context: FiveStarContext): boolean { return context === 'heli' || context === 'plane'; }
export function fiveStarSafeRect(width: number, height: number, insets: FiveStarInsets): FiveStarRect {
  return { x: insets.left, y: insets.top, width: Math.max(1, width - insets.left - insets.right),
    height: Math.max(1, height - insets.top - insets.bottom) };
}
export function fiveStarScale(safe: FiveStarRect, settings: FiveStarSettings): number {
  return Math.max(0.8, Math.min(1.25, Math.min(safe.width / 832, safe.height / 407))) * settings.controlScale;
}
export function fiveStarContains(frame: FiveStarRect, point: FiveStarPoint, slop: number = 0): boolean {
  return point.x >= frame.x - slop && point.x <= frame.x + frame.width + slop &&
    point.y >= frame.y - slop && point.y <= frame.y + frame.height + slop;
}
export function createFiveStarButtons(): FiveStarButton[] {
  const definitions: string[][] = [
    ["fire", "fire", "Fire"],
    ["aim", "aim", "Aim"],
    ["leftFire", "fire", "Fire, left hand"],
    ["jump", "jump", "Jump"],
    ["sprint", "sprint", "Sprint"],
    ["enter", "enter_exit", "Enter vehicle"],
    ["weapons", "weapons", "Weapon wheel"],
    ["reload", "reload", "Reload"],
    ["cover", "cover", "Take cover"],
    ["crouch", "crouch", "Stealth"],
    ["switchCharacter", "switch_character", "Switch character"],
    ["special", "special_ability", "Special ability"],
    ["interact", "interact", "Interact"],
    ["detonate", "detonate", "Detonate sticky bombs"],
    ["zoomIn", "zoom_in", "Zoom in"],
    ["zoomOut", "zoom_out", "Zoom out"],
    ["weaponPrev", "weapon_prev", "Previous weapon in this slot"],
    ["weaponNext", "weapon_next", "Next weapon in this slot"],
    ["gas", "gas", "Gas"],
    ["brake", "brake", "Brake and reverse"],
    ["handbrake", "handbrake", "Handbrake"],
    ["steer_left", "steer_left", "Steer left"],
    ["steer_right", "steer_right", "Steer right"],
    ["driveBy", "fire", "Aim and shoot from vehicle"],
    ["driveAim", "aim", "Aim from the vehicle"],
    ["duck", "duck", "Duck"],
    ["horn", "horn", "Horn"],
    ["lookBack", "look_back", "Look behind"],
    ["vehicleWeapon", "vehicle_weapon", "Next vehicle weapon"],
    ["towHook", "tow_hook", "Release tow hook"],
    ["roof", "roof", "Raise or lower the roof"],
    ["detachTrailer", "tow_hook", "Detach trailer"],
    ["towArmUp", "tow_arm_up", "Raise tow arm"],
    ["towArmDown", "tow_arm_down", "Lower tow arm"],
    ["dive", "dive", "Dive"],
    ["surface", "surface", "Surface"],
    ["airFire", "special", "Fire aircraft weapon"],
    ["gear", "landing_gear", "Landing gear"],
    ["throttleUp", "throttle_up", "Throttle up, climb"],
    ["throttleDown", "throttle_down", "Throttle down, descend"],
    ["yawLeft", "yaw_left", "Rudder left"],
    ["yawRight", "yaw_right", "Rudder right"],
    ["camera", "camera", "Change camera"],
    ["phone", "phone", "Phone"],
    ["pause", "pause", "Pause menu"],
    ["settings", "settings", "Settings"],
    ["mods", "mods", "Mods"],
    ["lights", "lights", "Headlights"],
    ["radio", "radio", "Radio"],
    ["up", "up", "Up"],
    ["down", "down", "Down"],
    ["left", "left", "Left"],
    ["right", "right", "Right"],
    ["accept", "accept", "Select"],
    ["back", "back", "Back"],
    ["phone_option", "phone_option", "Phone options"],
    ["taxi_hurry", "taxi_hurry", "Hurry up"],
    ["taxi_stop", "taxi_stop", "Stop the taxi"],
    ["prompt_option", "prompt_option", "Skip mission or other option (X)"],
    ["script_ltrt", "script_ltrt", "LT and RT together (tap to hold, tap again to let go)"],
    ["tabLeft", "tab_left", "Previous tab"],
    ["tabRight", "tab_right", "Next tab"],
    ["script_lb", "script_lb", "LB"],
    ["script_lt", "script_lt", "LT"],
    ["script_rt", "script_rt", "RT"],
    ["script_rb", "script_rb", "RB"],
    ["script_x", "script_x", "X"],
    ["script_y", "script_y", "Y"],
    ["script_a", "script_a", "A"],
    ["script_b", "script_b", "B"],
    ["script_ls", "script_ls", "Left stick click"],
    ["script_rs", "script_rs", "Right stick click"],
  ];
  return definitions.map((d: string[]): FiveStarButton => ({ name: d[0], icon: d[1], label: d[2],
    frame: { x: 0, y: 0, width: 44, height: 44 }, pressed: false, active: false, progress: 0,
    pedal: ['gas', 'brake', 'throttleUp', 'throttleDown', 'dive', 'surface'].indexOf(d[0]) >= 0,
    slideTarget: ['gas', 'brake', 'handbrake', 'throttleUp', 'throttleDown', 'dive', 'surface'].indexOf(d[0]) >= 0,
    hidden: false, opacity: 0.8 }));
}
export function layoutFiveStarButtons(buttons: FiveStarButton[], visible: string[], width: number, height: number,
  insets: FiveStarInsets, settings: FiveStarSettings, context: FiveStarContext, phoneDock: boolean): void {
  const safe: FiveStarRect = fiveStarSafeRect(width, height, insets);
  const scale: number = fiveStarScale(safe, settings);
  function place(name: string, fx: number, fy: number, size: number, pedalHeight: number = 0): void {
    const b: FiveStarButton | undefined = buttons.find((item: FiveStarButton): boolean => item.name === name);
    if (!b) { return; }
    const w: number = pedalHeight > 0 ? size * scale : Math.max(44, size * scale);
    const h: number = pedalHeight > 0 ? pedalHeight * scale : w;
    b.frame = { x: safe.x + fx * safe.width - w / 2, y: safe.y + fy * safe.height - h / 2, width: w, height: h };
  }
  const bar: string[] = ['radio', 'lights', 'camera', 'phone', 'pause', 'settings', 'mods'].filter((n: string): boolean => visible.indexOf(n) >= 0);
  for (let i: number = 0; i < bar.length; i++) {
    place(bar[i], 0.5 + (i - (bar.length - 1) / 2) * 54 * scale / safe.width, 0.08, 40);
  }
  place("fire", 0.885, 0.7, 78.0);
  place("aim", 0.735, 0.76, 62.0);
  place("jump", 0.8, 0.43, 56.0);
  place("sprint", 0.7, 0.3, 50.0);
  place("enter", 0.955, 0.42, 54.0);
  place("weapons", 0.955, 0.17, 50.0);
  place("reload", 0.69, 0.5, 50.0);
  place("cover", 0.6, 0.82, 52.0);
  place("crouch", 0.58, 0.6, 44.0);
  place("leftFire", 0.1, 0.36, 58.0);
  place("switchCharacter", 0.955, 0.295, 46.0);
  place("special", 0.86, 0.24, 48.0);
  place("interact", 0.6, 0.38, 52.0);
  place("detonate", 0.52, 0.68, 48.0);
  place("zoomIn", 0.66, 0.24, 44.0);
  place("zoomOut", 0.74, 0.24, 44.0);
  place("gas", 0.905, 0.72, 74.0, 108.0);
  place("brake", 0.775, 0.8, 74.0, 78.0);
  place("handbrake", 0.79, 0.52, 56.0);
  place("steer_left", 0.08, 0.76, 80.0);
  place("steer_right", 0.215, 0.76, 80.0);
  place("driveBy", 0.655, 0.5, 58.0);
  place("driveAim", 0.655, 0.71, 50.0);
  place("duck", 0.555, 0.5, 46.0);
  place("horn", 0.06, 0.36, 46.0);
  place("lookBack", 0.155, 0.36, 44.0);
  place("vehicleWeapon", 0.955, 0.17, 48.0);
  place("towHook", 0.655, 0.27, 50.0);
  place("roof", 0.575, 0.27, 50.0);
  place("detachTrailer", 0.655, 0.27, 50.0);
  place("towArmUp", 0.575, 0.27, 46.0);
  place("towArmDown", 0.495, 0.27, 46.0);
  place("surface", 0.655, 0.64, 62.0, 66.0);
  place("dive", 0.655, 0.84, 62.0, 66.0);
  place("airFire", 0.8, 0.3, 62.0);
  place("gear", 0.655, 0.24, 46.0);
  place("throttleUp", 0.905, 0.56, 74.0, 96.0);
  place("throttleDown", 0.905, 0.85, 74.0, 64.0);
  place("yawLeft", 0.655, 0.86, 54.0);
  place("yawRight", 0.775, 0.86, 54.0);
  place("accept", 0.905, 0.7, 68.0);
  place("back", 0.765, 0.84, 56.0);
  place("phone_option", 0.765, 0.56, 56.0);
  place("taxi_hurry", 0.765, 0.52, 58.0);
  place("taxi_stop", 0.765, 0.84, 58.0);
  place("prompt_option", 0.765, 0.56, 56.0);
  const centerX: number = (width / 2 - safe.x) / safe.width;
  const centerY: number = (height / 2 - safe.y) / safe.height;
  const gap: number = 34 * scale / safe.width;
  place('weaponPrev', centerX - gap, centerY, 50); place('weaponNext', centerX + gap, centerY, 50);
  const stepX: number = 58 * scale / safe.width, stepY: number = 58 * scale / safe.height;
  place('up', 0.12, 0.70 - stepY, 52); place('down', 0.12, 0.70 + stepY, 52);
  place('left', 0.12 - stepX, 0.70, 52); place('right', 0.12 + stepX, 0.70, 52);
  const missionNames: string[] = ['script_ltrt', 'script_lb', 'script_lt', 'script_rt', 'script_rb', 'script_x',
    'script_y', 'script_a', 'script_b', 'script_ls', 'script_rs'].filter((n: string): boolean => visible.indexOf(n) >= 0);
  for (let i: number = 0; i < missionNames.length; i++) {
    place(missionNames[i], 0.5 + (i - (missionNames.length - 1) / 2) * 60 * scale / safe.width, 0.27, 52);
  }
  place('tabLeft', 0.05, 0.12, 46); place('tabRight', 0.95, 0.12, 46);
  if (context !== 'menu' && !phoneDock) {
    const group: string = context === 'onFoot' ? 'foot' : 'vehicle';
    for (const button of buttons) {
      if (visible.indexOf(button.name) < 0) { continue; }
      const entry: FiveStarLayoutEntry | undefined = settings.layoutEntries[group + '.' + button.name];
      if (!entry) { continue; }
      const w: number = Math.max(36, button.frame.width * entry.scale);
      const h: number = Math.max(36, button.frame.height * entry.scale);
      button.frame = { x: safe.x + entry.x * safe.width - w / 2, y: safe.y + entry.y * safe.height - h / 2, width: w, height: h };
    }
  }
}
