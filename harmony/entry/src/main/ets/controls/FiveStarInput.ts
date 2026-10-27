// Adapted from Five Star Port 8e2e46281159d09005fea4748b5b9926e919fc0a.
// Copyright (c) 2026 GTA V iOS port contributors. Original source MIT;
// see upstream/fivestar-input-8e2e4628/LICENSE.
// HarmonyOS adaptation distributed under GPL-3.0-or-later.
// UIKit rendering, haptics and native calls are injected by the Harmony view/bridge.
// All times supplied here are monotonic milliseconds; elapsed is seconds, as upstream.
import { FiveStarSettings, FiveStarLayoutEntry, defaultFiveStarSettings, sanitizeFiveStarSettings } from './FiveStarSettings';
import { FiveStarPoint, FiveStarRect, FiveStarInsets, FiveStarContext, FiveStarButton, FiveStarStickDisplay,
  FIVE_STAR_ESSENTIAL, createFiveStarButtons, fiveStarContext, fiveStarDriving, fiveStarFlying,
  fiveStarSafeRect, fiveStarScale, fiveStarContains, layoutFiveStarButtons } from './FiveStarLayout';

export const FIVE_STAR_PAD = { l2: 1, r2: 2, l1: 4, r1: 8, y: 16, b: 32, a: 64, x: 128,
  select: 256, l3: 512, r3: 1024, start: 2048, up: 4096, right: 8192, down: 16384, left: 32768 };
export interface FiveStarHints {
  specialShown: boolean; specialReady: boolean; specialActive: boolean; specialFill: number;
  canDetonate: boolean; helpDetonate: boolean; helpShown: boolean; helpContext: boolean; helpInputs: number;
  radioWheelOpen: boolean; vehicleModel: number; pedModel: number; trailerAttached: boolean;
  passenger: boolean; taxiMenu: boolean; phoneBrowser: boolean;
}
export interface FiveStarSnapshot {
  hookReady?: boolean; known?: boolean; generation?: number; seq?: number; ageMs?: number;
  vehicle: number; pauseMenu: boolean; status: number; characterSwitchEnabled: boolean;
  hints: FiveStarHints; missionInputs: number[];
}
export interface FiveStarOutput {
  active: boolean; buttons: number; leftX: number; leftY: number; rightX: number; rightY: number;
  leftTrigger: number; rightTrigger: number;
  mouseDx: number; mouseDy: number; mouseButtons: number; shift: boolean; mouseActive: boolean;
  cameraSwipeX: number; cameraSwipeY: number;
  radioX: number; radioY: number; radioActive: boolean; mouseWheel: number;
  nextAircraftWeapon: boolean; openSettings: boolean; openMods: boolean; haptic: boolean;
}
export function defaultFiveStarHints(): FiveStarHints {
  return { specialShown: false, specialReady: false, specialActive: false, specialFill: 0,
    canDetonate: false, helpDetonate: false, helpShown: false, helpContext: false, helpInputs: 0,
    radioWheelOpen: false, vehicleModel: 0, pedModel: 0, trailerAttached: false,
    passenger: false, taxiMenu: false, phoneBrowser: false };
}
export function defaultFiveStarSnapshot(): FiveStarSnapshot {
  return { hookReady: false, known: false, vehicle: 0, pauseMenu: false, status: 16, characterSwitchEnabled: false,
    hints: defaultFiveStarHints(), missionInputs: [] };
}
export function parseFiveStarSnapshot(json: string): FiveStarSnapshot {
  try {
    const value: FiveStarSnapshot = JSON.parse(json) as FiveStarSnapshot;
    if (!value || !Number.isInteger(value.vehicle) || value.vehicle < 0 || value.vehicle > 32 ||
      !Number.isInteger(value.status) || value.status < 0 || typeof value.pauseMenu !== 'boolean' ||
      !Array.isArray(value.missionInputs)) {
      return defaultFiveStarSnapshot();
    }
    const hints: FiveStarHints = value.hints || defaultFiveStarHints();
    const number = (input: number | undefined, fallback: number, low: number, high: number): number =>
      typeof input === 'number' && Number.isFinite(input) ? Math.max(low, Math.min(high, input)) : fallback;
    const integer = (input: number | undefined, fallback: number, low: number, high: number): number =>
      Math.floor(number(input, fallback, low, high));
    const inputs: number[] = [];
    for (const input of value.missionInputs) {
      if (Number.isInteger(input) && input >= 0 && input < 400 && inputs.indexOf(input) < 0) { inputs.push(input); }
    }
    // Rust publishes a nullable age before the first hook. Null must never become a fresh 0 ms.
    return { hookReady: value.hookReady === true, known: value.known === true,
      generation: integer(value.generation, -1, 0, Number.MAX_SAFE_INTEGER),
      seq: integer(value.seq, 0, 0, Number.MAX_SAFE_INTEGER),
      ageMs: number(value.ageMs, Number.POSITIVE_INFINITY, 0, Number.MAX_SAFE_INTEGER),
      vehicle: value.vehicle, pauseMenu: value.pauseMenu, status: value.status,
      characterSwitchEnabled: value.characterSwitchEnabled === true, missionInputs: inputs,
      hints: { specialShown: hints.specialShown === true, specialReady: hints.specialReady === true,
        specialActive: hints.specialActive === true, specialFill: number(hints.specialFill, 0, 0, 1),
        canDetonate: hints.canDetonate === true, helpDetonate: hints.helpDetonate === true,
        helpShown: hints.helpShown === true, helpContext: hints.helpContext === true,
        helpInputs: integer(hints.helpInputs, 0, 0, 0x3FFFF), radioWheelOpen: hints.radioWheelOpen === true,
        vehicleModel: integer(hints.vehicleModel, 0, 0, 0xFFFFFFFF), pedModel: integer(hints.pedModel, 0, 0, 0xFFFFFFFF),
        trailerAttached: hints.trailerAttached === true, passenger: hints.passenger === true,
        taxiMenu: hints.taxiMenu === true, phoneBrowser: hints.phoneBrowser === true } };
  } catch (_) { return defaultFiveStarSnapshot(); }
}
export function emptyFiveStarOutput(): FiveStarOutput {
  return { active: false, buttons: 0, leftX: 0, leftY: 0, rightX: 0, rightY: 0, leftTrigger: 0, rightTrigger: 0,
    mouseDx: 0, mouseDy: 0, mouseButtons: 0, shift: false, mouseActive: false, cameraSwipeX: 0, cameraSwipeY: 0,
    radioX: 0, radioY: 0, radioActive: false, mouseWheel: 0, nextAircraftWeapon: false,
    openSettings: false, openMods: false, haptic: false };
}
interface MissionButton { input: number; name: string; pad: number; trigger: number; menuInput: number; }
const MISSION_BUTTONS: MissionButton[] = [
  { input: 226, name: 'script_lb', pad: 4, trigger: 0, menuInput: 205 },
  { input: 228, name: 'script_lt', pad: 0, trigger: 1, menuInput: 207 },
  { input: 229, name: 'script_rt', pad: 0, trigger: 2, menuInput: 208 },
  { input: 227, name: 'script_rb', pad: 8, trigger: 0, menuInput: 206 },
  { input: 224, name: 'script_x', pad: 128, trigger: 0, menuInput: -1 },
  { input: 222, name: 'script_y', pad: 16, trigger: 0, menuInput: 204 },
  { input: 223, name: 'script_a', pad: 64, trigger: 0, menuInput: -1 },
  { input: 225, name: 'script_b', pad: 32, trigger: 0, menuInput: -1 },
  { input: 230, name: 'script_ls', pad: 512, trigger: 0, menuInput: -1 },
  { input: 231, name: 'script_rs', pad: 1024, trigger: 0, menuInput: -1 }
];
const TOW_TRUCKS: number[] = [0xB12314E0, 0xE5A2D6C6];
const ARM_VEHICLES: number[] = [0xB12314E0, 0xE5A2D6C6, 0x1A7FCEFA];
const CARGOBOBS: number[] = [0xFCFCB68B, 0x60A7EA10, 0x53174EEF, 0x78BC1A3C];
const CONVERTIBLES: number[] = [0xA8E38B01, 0x7B8AB45F, 0x13B57D8A, 0xFAAD85EE, 0xB9CB3B69,
  0x679450AF, 0x3412AE2D, 0xB8E2AE18, 0x16E478C1, 0x5B42A5C4];
interface FingerRole {
  id: number; kind: string; button: string; start: FiveStarPoint; current: FiveStarPoint; since: number;
}
interface PadPulse { buttons: number; until: number; }
interface PadPress { bit: number; at: number; }
interface MapTap { id: number; since: number; at: FiveStarPoint; }
function clamp(value: number, low: number, high: number): number { return Math.max(low, Math.min(high, value)); }
function magnitude(p: FiveStarPoint): number { return Math.hypot(p.x, p.y); }
function hasAny(values: number[], other: number[]): boolean { return other.some((n: number): boolean => values.indexOf(n) >= 0); }
function hasAll(values: number[], other: number[]): boolean { return other.every((n: number): boolean => values.indexOf(n) >= 0); }
function signedPower(v: number, power: number): number { return Math.sign(v) * Math.pow(Math.abs(v), power); }

class FiveStarStick {
  origin: FiveStarPoint = { x: 0, y: 0 };
  value: FiveStarPoint = { x: 0, y: 0 };
  engaged: boolean = false;
  sprinting: boolean = false;
  diameter: number = 120;
  travel: number = 48;
  idle: FiveStarPoint | undefined = undefined;
  begin(point: FiveStarPoint): void { this.engaged = true; this.origin = point; this.value = { x: 0, y: 0 }; }
  move(point: FiveStarPoint): void {
    const dx: number = point.x - this.origin.x, dy: number = point.y - this.origin.y;
    const length: number = Math.max(1, Math.hypot(dx, dy) / this.travel);
    this.value = { x: dx / this.travel / length, y: -dy / this.travel / length };
  }
  end(): void { this.engaged = false; this.value = { x: 0, y: 0 }; this.sprinting = false; }
  display(name: string): FiveStarStickDisplay {
    const origin: FiveStarPoint = this.engaged ? this.origin : this.idle || this.origin;
    return { id: name, name: name, origin: origin,
      knob: { x: origin.x + this.value.x * this.travel, y: origin.y - this.value.y * this.travel },
      baseDiameter: this.diameter, x: origin.x, y: origin.y, valueX: this.value.x, valueY: this.value.y,
      diameter: this.diameter, travel: this.travel, visible: this.engaged || this.idle !== undefined,
      engaged: this.engaged, sprinting: this.sprinting, opacity: this.engaged ? 1 : 0.55 };
  }
}

export class FiveStarInput {
  editing: boolean = false;
  modsActive: boolean = false;
  private preferences: FiveStarSettings;
  private currentContext: FiveStarContext = 'onFoot';
  private allButtons: FiveStarButton[] = createFiveStarButtons();
  private roles: FingerRole[] = [];
  private moveStick: FiveStarStick = new FiveStarStick();
  private lookStick: FiveStarStick = new FiveStarStick();
  private rightStick: FiveStarStick = new FiveStarStick();
  private width: number = 832;
  private height: number = 407;
  private insets: FiveStarInsets = { left: 0, right: 0, top: 0, bottom: 0 };
  private hints: FiveStarHints = defaultFiveStarHints();
  private polledMission: number[] = [];
  private characterSwitchEnabled: boolean = false;
  private gamePhoneOut: boolean = false;
  private phoneDock: boolean = false;
  private phoneRequestedAt: number = -1;
  private phoneOpensDock: boolean = false;
  private lastDockInput: number = 0;
  private gamePaused: boolean = false;
  private promptActive: boolean = false;
  private promptSince: number = -1;
  private aimToggled: boolean = false;
  private sprintLatched: boolean = false;
  private sprintSince: number = -1;
  private sprintClock: number = 0;
  private pedalClock: number = 0;
  private fireStartedAt: number = -1;
  private exitHoldStart: number = -1;
  private planeThrottle: number = 0;
  private pulses: PadPulse[] = [];
  private pressedAt: PadPress[] = [];
  private lastHeld: number = 0;
  private detachTrailerWasPressed: boolean = false;
  private bothTriggersHeld: boolean = false;
  private bothTriggersWasPressed: boolean = false;
  private latchedTriggers: number[] = [];
  private triggerWasPressed: number[] = [];
  private cameraTravel: FiveStarPoint = { x: 0, y: 0 };
  private fractionalMouse: FiveStarPoint = { x: 0, y: 0 };
  private cameraMode: number = 0;
  private pendingSettings: boolean = false;
  private pendingMods: boolean = false;
  private pendingWeapon: boolean = false;
  private pendingHaptic: boolean = false;
  private pendingWheel: number = 0;
  private pinchNotchScale: number = 1;
  private mapRect: FiveStarRect | undefined = undefined;
  private mapTaps: MapTap[] = [];
  private fullMapRequestedAt: number = -1;
  private known: boolean = false;
  private arrivedAt: number = -1;
  private ageAtArrival: number = 0;
  private acceptedGeneration: number = -1;
  private acceptedSequence: number = -1;

  constructor(settings?: FiveStarSettings) { this.preferences = sanitizeFiveStarSettings(settings); this.refresh(); }
  get context(): FiveStarContext { return this.currentContext; }
  get aiming(): boolean { return this.aimToggled || this.down('fire') || this.down('leftFire') || this.down('driveBy'); }
  get showingPrompt(): boolean { return this.promptActive; }
  get phoneDockOpen(): boolean { return this.phoneDock; }
  get flyingWithRightStick(): boolean { return this.hasRole('planeStick'); }
  get layoutGroup(): string { return this.currentContext === 'onFoot' ? 'foot' : 'vehicle'; }
  getSettings(): FiveStarSettings { return this.preferences; }
  getLayoutEntry(name: string): FiveStarLayoutEntry | undefined { return this.preferences.layoutEntries[this.layoutGroup + '.' + name]; }
  setLayoutEntry(name: string, entry: FiveStarLayoutEntry): void {
    this.preferences.layoutEntries[this.layoutGroup + '.' + name] = { x: clamp(entry.x, 0, 1), y: clamp(entry.y, 0, 1),
      scale: clamp(entry.scale, 0.6, 1.8), hidden: entry.hidden && FIVE_STAR_ESSENTIAL.indexOf(name) < 0 };
    this.refresh();
  }
  resetLayoutGroup(): void {
    const prefix: string = this.layoutGroup + '.';
    const entries: Record<string, FiveStarLayoutEntry> = {};
    for (const key of Object.keys(this.preferences.layoutEntries)) {
      if (key.indexOf(prefix) !== 0) { entries[key] = this.preferences.layoutEntries[key]; }
    }
    this.preferences.layoutEntries = entries;
    this.refresh();
  }
  setSettings(settings: FiveStarSettings): void { this.preferences = sanitizeFiveStarSettings(settings); this.refresh(); }
  setViewport(width: number, height: number, insets?: FiveStarInsets): void {
    if (Number.isFinite(width) && width > 0) { this.width = width; }
    if (Number.isFinite(height) && height > 0) { this.height = height; }
    if (insets) { this.insets = insets; }
    this.refresh();
  }
  // The host supplies the actual relocated minimap rectangle (including its HUD transformation).
  setMinimapRect(rect?: FiveStarRect): void { this.mapRect = rect; }
  setEditorContext(context: FiveStarContext): void {
    this.resetTouches(false); this.phoneDock = false; this.promptActive = false;
    this.currentContext = context; this.refresh();
  }
  normalized(point: FiveStarPoint): FiveStarPoint {
    const safe: FiveStarRect = this.safe();
    return { x: (point.x - safe.x) / safe.width, y: (point.y - safe.y) / safe.height };
  }
  clampedToSafeArea(point: FiveStarPoint): FiveStarPoint {
    const safe: FiveStarRect = this.safe();
    return { x: clamp(point.x, safe.x + 24, safe.x + safe.width - 24),
      y: clamp(point.y, safe.y + 24, safe.y + safe.height - 24) };
  }
  visibleButtons(): FiveStarButton[] {
    const names: string[] = this.refresh();
    return names.map((name: string): FiveStarButton => this.button(name));
  }
  sticks(): FiveStarStickDisplay[] { this.refresh(); return [this.moveStick.display('move'), this.lookStick.display('look'), this.rightStick.display('plane')]; }
  button(name: string): FiveStarButton {
    const found: FiveStarButton | undefined = this.allButtons.find((b: FiveStarButton): boolean => b.name === name);
    if (!found) { throw new Error('Unknown Five Star control: ' + name); }
    return found;
  }
  editableButton(point: FiveStarPoint): FiveStarButton | undefined { return this.buttonAt(point); }
  private safe(): FiveStarRect { return fiveStarSafeRect(this.width, this.height, this.insets); }
  private down(name: string): boolean { return this.button(name).pressed; }
  private setDown(name: string, value: boolean): void { this.button(name).pressed = value; }
  private hasRole(kind: string): boolean { return this.roles.some((r: FingerRole): boolean => r.kind === kind); }
  private hasSelectionWheel(): boolean { return this.roles.some((r: FingerRole): boolean => ['weaponWheel', 'radioWheel', 'characterWheel'].indexOf(r.kind) >= 0); }
  private helpAsked(): number[] {
    const result: number[] = [];
    if (this.hints.helpShown) {
      for (let bit: number = 0; bit < 18; bit++) { if ((this.hints.helpInputs & (1 << bit)) !== 0) { result.push(218 + bit); } }
    }
    return result;
  }
  private scriptMenu(): boolean { return hasAll(this.polledMission, [187, 188]) && this.currentContext !== 'menu' && !this.phoneDock; }
  private taxiRide(): boolean { return this.hints.passenger && this.hints.taxiMenu && this.hints.vehicleModel === 0xC703DB5F; }
  private tvCouch(): boolean {
    return this.currentContext === 'onFoot' && !this.promptActive && !this.scriptMenu() && !this.taxiRide() && hasAny(this.helpAsked(), [218, 219]);
  }
  private planeStickEnabled(): boolean {
    return this.currentContext === 'plane' && this.preferences.airplaneRightStick && !this.phoneDock && !this.promptActive && !this.editing;
  }
  private swipeEnabled(): boolean {
    return this.preferences.cameraTouchMode !== 0 && this.currentContext !== 'menu' && !this.phoneDock && !this.gamePhoneOut &&
      !this.promptActive && !fiveStarFlying(this.currentContext) && !this.hints.phoneBrowser && !hasAny(this.helpAsked(), [220, 221]);
  }
  private mouseLookActive(): boolean { return this.swipeEnabled() && this.preferences.cameraTouchMode === 2 && !this.hasSelectionWheel(); }

  update(snapshot: FiveStarSnapshot, nowMs: number): void {
    if (snapshot.generation !== undefined && snapshot.generation < this.acceptedGeneration) { return; }
    if (snapshot.generation === this.acceptedGeneration && snapshot.seq !== undefined && snapshot.seq < this.acceptedSequence) { return; }
    if (snapshot.generation !== undefined && this.acceptedGeneration >= 0 && snapshot.generation !== this.acceptedGeneration) { this.reset(); }
    if (snapshot.generation !== undefined) { this.acceptedGeneration = snapshot.generation; }
    if (snapshot.seq !== undefined) { this.acceptedSequence = snapshot.seq; }
    this.arrivedAt = nowMs;
    this.ageAtArrival = Math.max(0, snapshot.ageMs || 0);
    this.known = (snapshot.hookReady === true || (snapshot.hookReady === undefined && snapshot.known !== false)) && this.ageAtArrival <= 1500;
    if (!this.known) { this.reset(); return; }
    // A live engine without a local ped still needs frontend navigation; world readiness and
    // the input hook are separate observations. The layout editor owns its preview context.
    if (this.editing) { this.refresh(); return; }
    this.characterSwitchEnabled = snapshot.characterSwitchEnabled;
    this.gamePaused = (snapshot.status & 32) !== 0;
    const phoneOut: boolean = (snapshot.status & 2) !== 0;
    if (phoneOut !== this.gamePhoneOut) {
      this.gamePhoneOut = phoneOut;
      const dockWas: boolean = this.phoneDock;
      if (phoneOut) { this.phoneRequestedAt = -1; this.lastDockInput = nowMs; }
      else if (this.phoneDock) { this.phoneDock = false; }
      if (this.currentContext !== 'menu' && this.phoneDock !== dockWas) { this.resetTouches(false); }
    }
    const paused: boolean = (snapshot.status & 32) !== 0 && this.currentContext !== 'menu';
    const wanted: boolean = (snapshot.status & 16) === 0 &&
      (((snapshot.status & 1) !== 0 && (snapshot.status & 4) === 0) || (snapshot.status & 8) !== 0 || paused);
    if (wanted) { if (this.promptSince < 0) { this.promptSince = nowMs; } } else { this.promptSince = -1; }
    const prompt: boolean = wanted && nowMs - this.promptSince > 600;
    if (prompt !== this.promptActive) { this.promptActive = prompt; this.resetTouches(false); }
    const next: FiveStarContext = snapshot.known === false ? 'menu' : fiveStarContext(snapshot.vehicle, snapshot.pauseMenu);
    if (next !== this.currentContext) {
      const carry: boolean = !fiveStarFlying(next) && next !== 'menu' && (fiveStarDriving(next) !== fiveStarDriving(this.currentContext));
      this.resetTouches(carry); this.currentContext = next;
      if (next === 'menu' || !this.gamePhoneOut) { this.phoneDock = false; }
    }
    this.hints = snapshot.hints;
    this.polledMission = snapshot.missionInputs.slice();
    for (const input of this.helpAsked()) { if (this.polledMission.indexOf(input) < 0) { this.polledMission.push(input); } }
    this.button('special').active = this.hints.specialActive;
    this.button('special').progress = this.hints.specialActive ? 0 : clamp(this.hints.specialFill, 0, 1);
    this.refresh();
  }

  private baseVisible(): string[] {
    const c: FiveStarContext = this.currentContext;
    const utility: string[] = fiveStarDriving(c) ? ['radio', 'lights', 'camera', 'phone', 'pause', 'settings'] : ['camera', 'phone', 'pause', 'settings'];
    const steering: string[] = this.preferences.steeringButtons ? ['steer_left', 'steer_right'] : [];
    if (c === 'menu') { return ['pause', 'settings', 'up', 'down', 'left', 'right', 'accept', 'back', 'tabLeft', 'tabRight']; }
    if (this.phoneDock) { return ['camera', 'phone', 'pause', 'settings', 'up', 'down', 'left', 'right', 'accept', 'back', 'phone_option']; }
    if ((this.promptActive || this.scriptMenu()) && !this.editing) {
      const stick: boolean = hasAny(this.polledMission, [218, 219, 220, 221]) && !this.scriptMenu() && !hasAny(this.helpAsked(), [232, 233, 234, 235]);
      return ['camera', 'pause', 'settings'].concat(stick ? [] : ['up', 'down', 'left', 'right'])
        .concat(this.hints.helpContext ? ['interact'] : []).concat(['accept', 'back', 'prompt_option']);
    }
    if (this.tvCouch() && !this.editing) { return ['camera', 'phone', 'pause', 'settings', 'up', 'down', 'left', 'right'].concat(this.hints.helpContext ? ['interact'] : []); }
    if (this.taxiRide() && !this.editing) { return ['camera', 'phone', 'pause', 'settings', 'radio', 'left', 'right', 'accept', 'taxi_hurry', 'taxi_stop', 'enter', 'lookBack']; }
    switch (c) {
      case 'onFoot': return utility.concat(this.aimToggled ? ['fire', 'aim', 'enter', 'weapons', 'reload', 'cover', 'leftFire'] :
        ['fire', 'aim', 'jump', 'enter', 'weapons', 'reload', 'cover', 'crouch'].concat(this.preferences.sprintButton ? ['sprint'] : []));
      case 'car': case 'boat': return utility.concat(['gas', 'brake', 'handbrake', 'enter', 'driveBy', 'driveAim', 'duck', 'horn', 'lookBack', 'vehicleWeapon']).concat(steering);
      case 'bike': return utility.concat(['gas', 'brake', 'handbrake', 'enter', 'driveBy', 'driveAim', 'duck', 'horn', 'lookBack']).concat(steering);
      case 'bicycle': return ['camera', 'phone', 'pause', 'settings', 'radio', 'gas', 'brake', 'handbrake', 'enter', 'lookBack'].concat(steering);
      case 'sub': return utility.concat(['gas', 'brake', 'enter', 'dive', 'surface', 'lookBack']);
      case 'heli': return utility.concat(['airFire', 'vehicleWeapon', 'enter', 'throttleUp', 'throttleDown', 'yawLeft', 'yawRight']);
      case 'plane': return utility.concat(this.planeStickEnabled() ? ['airFire', 'vehicleWeapon', 'gear', 'enter'] :
        ['airFire', 'vehicleWeapon', 'gear', 'enter', 'throttleUp', 'throttleDown', 'yawLeft', 'yawRight']);
      default: return [];
    }
  }
  private computeVisible(): string[] {
    let list: string[] = this.baseVisible();
    if ((this.modsActive || this.editing) && list.indexOf('settings') >= 0) { list.push('mods'); }
    if (!this.editing && this.currentContext !== 'menu' && !this.phoneDock && (this.promptActive || this.hints.helpShown || this.scriptMenu())) {
      const covered: number[] = [];
      if (list.indexOf('accept') >= 0) { covered.push(223); }
      if (list.indexOf('back') >= 0) { covered.push(225); }
      if (list.indexOf('prompt_option') >= 0) { covered.push(224); }
      const both: boolean = hasAll(this.polledMission, [228, 229]) || hasAll(this.polledMission, [207, 208]);
      if (both) { covered.push(228, 229); list.push('script_ltrt'); }
      for (const m of MISSION_BUTTONS) {
        if ((this.polledMission.indexOf(m.input) >= 0 || this.polledMission.indexOf(m.menuInput) >= 0) && covered.indexOf(m.input) < 0) { list.push(m.name); }
      }
    }
    const playing: boolean = !this.phoneDock && !this.promptActive && this.currentContext !== 'menu';
    const switchAsked: boolean = this.promptActive && !this.phoneDock &&
      (this.hints.pedModel === 0x14EC17EA || (this.polledMission.indexOf(19) >= 0 && !this.gamePaused));
    if ((this.characterSwitchEnabled || this.editing) && (playing || switchAsked) && this.currentContext !== 'menu') { list.push('switchCharacter'); }
    if (this.editing && !fiveStarFlying(this.currentContext)) {
      return list.concat(this.currentContext === 'onFoot' ? ['special', 'interact', 'detonate', 'zoomIn', 'zoomOut', 'weaponPrev', 'weaponNext'] :
        ['special', 'interact', 'detonate', 'towHook', 'towArmUp', 'towArmDown', 'roof']);
    }
    if (playing && !fiveStarFlying(this.currentContext)) {
      if (this.hints.specialShown && !this.aimToggled) { list.push('special'); }
      if (this.hints.canDetonate || this.hints.helpDetonate) { list.push('detonate'); }
      if (this.hints.helpContext && !this.aimToggled) { list.push('interact'); }
      if (this.currentContext === 'onFoot' && this.aimToggled) { list.push('zoomIn', 'zoomOut'); }
      if (this.currentContext === 'onFoot' && this.hasRole('weaponWheel')) { list.push('weaponPrev', 'weaponNext'); }
      if (this.currentContext === 'car') {
        if (TOW_TRUCKS.indexOf(this.hints.vehicleModel) >= 0) { list.push('towHook'); }
        if (ARM_VEHICLES.indexOf(this.hints.vehicleModel) >= 0) { list.push('towArmUp', 'towArmDown'); }
        if (CONVERTIBLES.indexOf(this.hints.vehicleModel) >= 0) { list.push('roof'); }
        if (this.hints.trailerAttached) { list.push('detachTrailer'); }
      }
    }
    if (playing && this.currentContext === 'heli' && CARGOBOBS.indexOf(this.hints.vehicleModel) >= 0) { list.push('towHook'); }
    return list;
  }
  private hiddenByPlayer(name: string): boolean { return this.currentContext !== 'menu' && !this.phoneDock && (this.getLayoutEntry(name)?.hidden || false); }
  private refresh(): string[] {
    if (!this.planeStickEnabled()) {
      this.rightStick.end(); this.planeThrottle = 0;
      this.roles = this.roles.filter((r: FingerRole): boolean => r.kind !== 'planeStick');
    }
    const list: string[] = this.computeVisible().filter((name: string): boolean => this.editing || !this.hiddenByPlayer(name));
    this.button('accept').icon = this.phoneDock && this.gamePhoneOut ? 'script_a' : 'accept';
    this.button('back').icon = this.phoneDock && this.gamePhoneOut ? 'script_b' : 'back';
    this.button('phone_option').icon = this.phoneDock && this.gamePhoneOut ? 'script_x' : 'phone_option';
    this.button('gas').icon = this.currentContext === 'bicycle' ? 'pedal' : 'gas';
    this.button('enter').label = this.currentContext === 'onFoot' ? 'Enter vehicle' : fiveStarFlying(this.currentContext) ? 'Hold to exit aircraft' : 'Exit vehicle';
    for (const button of this.allButtons) {
      button.hidden = this.hiddenByPlayer(button.name);
      button.opacity = this.editing && button.hidden ? 0.3 : button.name === 'fire' || button.name === 'gas' ? Math.min(1, this.preferences.controlOpacity + 0.1) : this.preferences.controlOpacity;
    }
    this.button('aim').active = this.aimToggled;
    this.button('sprint').active = this.sprintLatched;
    layoutFiveStarButtons(this.allButtons, list, this.width, this.height, this.insets, this.preferences, this.currentContext, this.phoneDock);
    const safe: FiveStarRect = this.safe(), scale: number = fiveStarScale(safe, this.preferences);
    this.lookStick.diameter = this.moveStick.diameter = 120 * scale;
    this.lookStick.travel = this.moveStick.travel = 48 * scale;
    this.rightStick.diameter = 128 * scale; this.rightStick.travel = 52 * scale;
    this.rightStick.idle = this.planeStickEnabled() ? { x: safe.x + safe.width * 0.80, y: safe.y + safe.height * 0.60 } : undefined;
    return list;
  }
  private buttonAt(point: FiveStarPoint, slidingOnly: boolean = false): FiveStarButton | undefined {
    const visible: string[] = this.refresh();
    let found: FiveStarButton | undefined = undefined;
    let nearest: number = Number.POSITIVE_INFINITY;
    for (const name of visible) {
      const b: FiveStarButton = this.button(name);
      if (slidingOnly && !b.slideTarget) { continue; }
      if (!fiveStarContains(b.frame, point, b.name === 'fire' || b.pedal ? 10 : 4)) { continue; }
      const distance: number = Math.hypot(b.frame.x + b.frame.width / 2 - point.x, b.frame.y + b.frame.height / 2 - point.y);
      if (distance < nearest) { nearest = distance; found = b; }
    }
    return found;
  }

  touchDown(id: number, x: number, y: number, nowMs: number): void {
    if (this.editing || !this.known || this.roles.some((r: FingerRole): boolean => r.id === id)) { return; }
    const point: FiveStarPoint = { x: x, y: y };
    const b: FiveStarButton | undefined = this.buttonAt(point);
    if (this.currentContext !== 'menu' && !this.phoneDock && !b && this.mapRect && fiveStarContains(this.mapRect, point, 8)) {
      this.mapTaps.push({ id: id, since: nowMs, at: point });
    }
    const safe: FiveStarRect = this.safe(), left: boolean = x < safe.x + safe.width * 0.45;
    const role: FingerRole = { id: id, kind: '', button: '', start: point, current: point, since: nowMs };
    if (b) { this.press(b, role, nowMs); return; }
    if (this.currentContext === 'menu' && left) {
      if (!this.hasRole('moveStick') && !this.hasRole('steer')) { role.kind = 'moveStick'; this.moveStick.begin(point); }
    } else if (this.currentContext === 'menu' || this.phoneDock) {
      if (!this.hasRole('look')) { role.kind = 'look'; this.lookStick.begin(point); }
    } else if (this.planeStickEnabled() && !left) {
      if (!this.hasRole('planeStick')) { role.kind = 'planeStick'; this.rightStick.begin(point); }
    } else if (fiveStarFlying(this.currentContext) && left) {
      if (!this.hasRole('moveStick') && !this.hasRole('steer')) { role.kind = 'moveStick'; this.moveStick.begin(point); }
    } else if (left && fiveStarDriving(this.currentContext) && this.preferences.steeringButtons) {
      // Upstream reserves the left zone for explicit steering buttons in this mode.
    } else if (left) {
      if (y > safe.y + safe.height * 0.2 && !this.hasRole('moveStick') && !this.hasRole('steer')) {
        role.kind = fiveStarDriving(this.currentContext) ? 'steer' : 'moveStick'; this.moveStick.begin(point);
      }
    } else if (!this.hasRole('look')) { role.kind = 'look'; if (!this.swipeEnabled()) { this.lookStick.begin(point); } }
    if (role.kind.length > 0) { this.roles.push(role); }
  }
  private press(button: FiveStarButton, role: FingerRole, nowMs: number): void {
    this.pendingHaptic = true; button.pressed = true; role.kind = 'button'; role.button = button.name;
    switch (button.name) {
      case 'weapons': this.clearCameraTravel(); role.kind = 'weaponWheel'; break;
      case 'radio': if (fiveStarDriving(this.currentContext)) { this.clearCameraTravel(); role.kind = 'radioWheel'; } break;
      case 'special': this.pulse(FIVE_STAR_PAD.l3 | FIVE_STAR_PAD.r3, 130, nowMs); break;
      case 'switchCharacter': this.clearCameraTravel(); role.kind = 'characterWheel'; break;
      case 'fire': case 'driveBy': role.kind = 'fireLook'; this.fireStartedAt = nowMs; break;
      case 'leftFire': role.kind = 'fireLook'; break;
      case 'sprint': this.sprintLatched = !this.sprintLatched; break;
      case 'aim': this.aimToggled = !this.aimToggled; role.kind = 'fireLook'; break;
      case 'enter': if (fiveStarFlying(this.currentContext)) { this.exitHoldStart = nowMs; } break;
      case 'phone':
        if (this.gamePhoneOut && !this.phoneDock) { this.phoneDock = true; this.phoneOpensDock = true; }
        else if (this.gamePhoneOut) { this.phoneRequestedAt = -1; }
        else { this.phoneDock = true; this.phoneRequestedAt = nowMs; }
        this.lastDockInput = nowMs; break;
      case 'settings': this.pendingSettings = true; break;
      case 'mods': this.pendingMods = true; break;
      case 'vehicleWeapon': if (fiveStarFlying(this.currentContext)) { this.pendingWeapon = true; } break;
      case 'roof': this.pulse(FIVE_STAR_PAD.right, 800, nowMs); break;
      case 'up': case 'down': case 'left': case 'right': case 'accept': case 'back': this.lastDockInput = nowMs; break;
      default: break;
    }
    this.roles.push(role); this.refresh();
  }
  touchMove(id: number, x: number, y: number, nowMs: number): void {
    const point: FiveStarPoint = { x: x, y: y };
    this.mapTaps = this.mapTaps.filter((tap: MapTap): boolean => tap.id !== id || Math.hypot(x - tap.at.x, y - tap.at.y) <= 12);
    const role: FingerRole | undefined = this.roles.find((r: FingerRole): boolean => r.id === id);
    if (!role || this.editing) { return; }
    const previous: FiveStarPoint = role.current;
    role.current = point;
    switch (role.kind) {
      case 'moveStick': case 'steer': this.moveStick.move(point); break;
      case 'planeStick':
        this.rightStick.move(point); this.planeThrottle = clamp(this.planeThrottle - (y - previous.y) / (this.rightStick.travel * 2), -1, 1); break;
      case 'look': if (this.swipeEnabled()) { this.trackCamera(previous, point); } else { this.lookStick.move(point); } break;
      case 'fireLook': if (role.button !== 'leftFire') { this.trackCamera(previous, point); } break;
      case 'button': {
        const button: FiveStarButton = this.button(role.button);
        if (button.slideTarget && !fiveStarContains(button.frame, point, 10)) {
          const next: FiveStarButton | undefined = this.buttonAt(point, true);
          if (next && next.name !== button.name) { button.pressed = false; next.pressed = true; role.button = next.name; this.pendingHaptic = true; }
        }
        break;
      }
      default: break;
    }
  }
  touchUp(id: number, nowMs: number): void {
    const tap: MapTap | undefined = this.mapTaps.find((m: MapTap): boolean => m.id === id);
    this.mapTaps = this.mapTaps.filter((m: MapTap): boolean => m.id !== id);
    if (tap && nowMs - tap.since < 300 && this.fullMapRequestedAt < 0) {
      this.fullMapRequestedAt = nowMs; this.pulse(FIVE_STAR_PAD.start, 120, nowMs);
    }
    const role: FingerRole | undefined = this.roles.find((r: FingerRole): boolean => r.id === id);
    this.roles = this.roles.filter((r: FingerRole): boolean => r.id !== id);
    if (!role) { return; }
    switch (role.kind) {
      case 'moveStick': case 'steer': this.moveStick.end(); this.sprintSince = -1; this.sprintLatched = false; break;
      case 'planeStick': this.rightStick.end(); break;
      case 'look': this.lookStick.end(); break;
      case 'weaponWheel': this.setDown('weapons', false); break;
      case 'characterWheel': this.setDown('switchCharacter', false); break;
      case 'radioWheel': this.setDown('radio', false); if (nowMs - role.since < 250) { this.pulse(FIVE_STAR_PAD.left, 100, nowMs); } break;
      case 'fireLook':
        this.setDown(role.button, false); if (role.button === 'fire' || role.button === 'driveBy') { this.fireStartedAt = -1; } break;
      case 'button':
        this.setDown(role.button, false); if (role.button === 'enter') { this.exitHoldStart = -1; this.button('enter').progress = 0; } break;
      default: break;
    }
    this.refresh();
  }
  // A platform gesture cancellation cancels all owned output, unlike a normal finger-up.
  touchCancel(): void { this.reset(); }
  beginMapPinch(): void { this.pinchNotchScale = 1; }
  mapPinch(scale: number): void {
    if (this.currentContext !== 'menu' || !Number.isFinite(scale) || scale <= 0) { return; }
    while (scale > this.pinchNotchScale * 1.15) { this.pendingWheel++; this.pinchNotchScale *= 1.15; }
    while (scale < this.pinchNotchScale / 1.15) { this.pendingWheel--; this.pinchNotchScale /= 1.15; }
  }
  private trackCamera(previous: FiveStarPoint, point: FiveStarPoint): void {
    if (!this.swipeEnabled() || this.hasSelectionWheel()) { return; }
    this.cameraTravel.x += point.x - previous.x; this.cameraTravel.y += point.y - previous.y;
  }
  private clearCameraTravel(): void { this.cameraTravel = { x: 0, y: 0 }; this.fractionalMouse = { x: 0, y: 0 }; }
  private pulse(buttons: number, durationMs: number, nowMs: number): void { this.pulses.push({ buttons: buttons, until: nowMs + durationMs }); }
  private resetTouches(keepStick: boolean): void {
    this.clearCameraTravel(); this.rightStick.end(); this.planeThrottle = 0;
    const keep: FingerRole[] = [];
    for (const role of this.roles) {
      if ((role.kind === 'moveStick' || role.kind === 'steer') && keepStick) {
        role.kind = fiveStarDriving(this.currentContext) ? 'moveStick' : 'steer'; keep.push(role); continue;
      }
      if (role.kind === 'moveStick' || role.kind === 'steer') { this.moveStick.end(); }
      if (role.kind === 'look') { this.lookStick.end(); }
      if (role.button.length > 0) { this.setDown(role.button, false); }
    }
    this.roles = keep; this.aimToggled = false; this.sprintLatched = false;
    this.fireStartedAt = -1; this.exitHoldStart = -1; this.sprintSince = -1;
    this.button('enter').progress = 0; this.moveStick.sprinting = false;
  }
  reset(): void {
    this.resetTouches(false); this.pulses = []; this.pressedAt = []; this.lastHeld = 0;
    this.latchedTriggers = []; this.triggerWasPressed = []; this.bothTriggersHeld = false;
    this.bothTriggersWasPressed = false; this.detachTrailerWasPressed = false;
    this.mapTaps = []; this.fullMapRequestedAt = -1; this.pendingWheel = 0;
    this.pendingSettings = false; this.pendingMods = false; this.pendingWeapon = false; this.pendingHaptic = false;
    this.sprintClock = 0; this.pedalClock = 0;
    for (const button of this.allButtons) { button.pressed = false; button.active = false; }
    this.refresh();
  }

  sample(nowMs: number, elapsedSeconds: number): FiveStarOutput {
    const out: FiveStarOutput = emptyFiveStarOutput();
    // Harmony's cached bridge can stop updating independently of rendering. Never keep an old pad held.
    if (!this.editing && (!this.known || this.arrivedAt < 0 || this.ageAtArrival + nowMs - this.arrivedAt > 1500)) {
      this.known = false; this.reset(); return out;
    }
    if (this.editing) { return out; }
    this.refresh();
    out.active = true;
    out.openSettings = this.pendingSettings; out.openMods = this.pendingMods;
    out.nextAircraftWeapon = this.pendingWeapon; out.haptic = this.pendingHaptic; out.mouseWheel = this.pendingWheel;
    this.pendingSettings = false; this.pendingMods = false; this.pendingWeapon = false; this.pendingHaptic = false; this.pendingWheel = 0;
    const elapsed: number = Number.isFinite(elapsedSeconds) ? Math.max(0, elapsedSeconds) : 0;
    this.flushCamera(out);
    let held: number = 0;
    let look: FiveStarPoint = this.hasRole('look') ? this.lookStick.value : { x: 0, y: 0 };
    for (const role of this.roles) {
      if (role.kind !== 'fireLook') { continue; }
      let value: FiveStarPoint = { x: (role.current.x - role.start.x) / this.lookStick.travel,
        y: -(role.current.y - role.start.y) / this.lookStick.travel };
      const length: number = magnitude(value);
      if (length > 1) { value = { x: value.x / length, y: value.y / length }; }
      if (magnitude(value) > magnitude(look)) { look = value; }
    }
    if (!this.swipeEnabled()) {
      const result: FiveStarPoint = FiveStarInput.cameraStick(look, this.aimToggled ? this.preferences.aimSensitivity : this.preferences.lookSensitivity);
      out.rightX = result.x; out.rightY = result.y;
    }
    const p = FIVE_STAR_PAD;
    const c: FiveStarContext = this.currentContext;
    if (c === 'onFoot') {
      out.leftX = this.moveStick.value.x; out.leftY = this.moveStick.value.y;
      if (this.down('sprint')) { held |= p.a; }
      const moving: number = magnitude(this.moveStick.value);
      if (this.sprintLatched && !this.promptActive && moving > 0.2) {
        this.sprintClock += elapsed; if (this.sprintClock % 0.3 < 0.13) { held |= p.a; }
        this.moveStick.sprinting = !this.preferences.sprintButton;
      } else if (!this.preferences.sprintButton && moving > 0.92 && !this.promptActive) {
        if (this.sprintSince < 0) { this.sprintSince = nowMs; this.sprintClock = 0; }
        if (nowMs - this.sprintSince > 250) {
          this.sprintClock += elapsed; if (this.sprintClock % 0.3 < 0.13) { held |= p.a; } this.moveStick.sprinting = true;
        }
      } else { this.sprintSince = -1; this.sprintClock = 0; this.moveStick.sprinting = false; }
      const firing: boolean = this.down('fire') || this.down('leftFire');
      if (this.aimToggled && this.moveStick.sprinting) { this.aimToggled = false; this.refresh(); }
      if (this.aimToggled || (firing && this.preferences.fireAlsoAims)) { out.leftTrigger = 1; }
      if (firing) {
        const delay: number = !this.aimToggled && this.preferences.fireAlsoAims && this.down('fire') ? 90 : 0;
        if (nowMs - (this.fireStartedAt < 0 ? 0 : this.fireStartedAt) >= delay || this.down('leftFire')) { out.rightTrigger = 1; }
      }
      const map: string[] = ['jump', 'enter', 'reload', 'cover', 'crouch', 'interact', 'zoomIn', 'zoomOut', 'weaponPrev', 'weaponNext'];
      const bits: number[] = [p.x, p.y, p.b, p.r1, p.l3, p.right, p.up, p.down, p.left, p.right];
      for (let i: number = 0; i < map.length; i++) { if (this.down(map[i])) { held |= bits[i]; } }
      for (const role of this.roles) {
        if (role.kind !== 'weaponWheel') { continue; }
        held |= p.l1; const d: FiveStarPoint = { x: role.current.x - role.start.x, y: role.current.y - role.start.y }, length: number = magnitude(d);
        out.rightX = length > 24 ? d.x / length : 0; out.rightY = length > 24 ? -d.y / length : 0;
      }
    } else if (fiveStarDriving(c)) {
      const steer: number = this.preferences.steeringButtons ? (this.down('steer_right') ? 1 : 0) - (this.down('steer_left') ? 1 : 0) : this.moveStick.value.x;
      out.leftX = signedPower(steer, 1.3);
      const tilt: number = this.moveStick.value.y;
      out.leftY = c === 'bike' || c === 'bicycle' ? tilt : Math.abs(tilt) < 0.35 ? 0 : Math.sign(tilt) * (Math.abs(tilt) - 0.35) / 0.65;
      if (c === 'bicycle') {
        if (this.down('gas')) { this.pedalClock += elapsed; if (this.pedalClock % 0.3 < 0.15) { held |= p.a; } } else { this.pedalClock = 0; }
        if (this.down('brake')) { out.leftTrigger = 1; } if (this.down('handbrake')) { out.rightTrigger = 1; }
      } else {
        if (this.down('gas')) { out.rightTrigger = 1; } if (this.down('brake')) { out.leftTrigger = 1; }
        if (this.down('handbrake')) { held |= p.r1; }
      }
      if (this.down('driveBy')) { held |= p.l1; if (this.fireStartedAt >= 0 && nowMs - this.fireStartedAt > 120) { held |= p.r1; } }
      const map: string[] = ['enter', 'driveAim', 'interact', 'horn', 'lookBack', 'vehicleWeapon', 'dive', 'surface', 'lights', 'towHook', 'detonate'];
      const bits: number[] = [p.y, p.l1, p.right, p.l3, p.r3, p.x, p.a, p.x, p.right, p.right, p.left];
      for (let i: number = 0; i < map.length; i++) { if (this.down(map[i])) { held |= bits[i]; } }
      if (this.down('duck') && c !== 'bicycle') { held |= p.a; }
      if (this.down('detachTrailer') && !this.detachTrailerWasPressed) { this.pulse(p.right, 1500, nowMs); }
      this.detachTrailerWasPressed = this.down('detachTrailer');
      if (this.down('towArmUp')) { out.leftY = 1; } else if (this.down('towArmDown')) { out.leftY = -1; }
      for (const role of this.roles) {
        if (role.kind !== 'radioWheel' || nowMs - role.since < 250) { continue; }
        held |= p.left;
        const d: FiveStarPoint = { x: role.current.x - this.width / 2, y: role.current.y - this.height / 2 }, length: number = magnitude(d);
        out.radioX = length > 40 ? d.x / length : 0; out.radioY = length > 40 ? -d.y / length : 0; out.radioActive = true;
        out.rightX = 0; out.rightY = 0; out.leftX = 0; out.leftY = 0;
      }
    } else if (fiveStarFlying(c)) {
      out.leftX = this.moveStick.value.x; out.leftY = this.moveStick.value.y;
      if (this.down('throttleUp')) { out.rightTrigger = 1; } if (this.down('throttleDown')) { out.leftTrigger = 1; }
      if (this.down('yawLeft')) { held |= p.l1; } if (this.down('yawRight')) { held |= p.r1; }
      if (this.planeStickEnabled()) {
        out.rightTrigger = Math.max(0, this.planeThrottle); out.leftTrigger = Math.max(0, -this.planeThrottle);
        if (this.rightStick.value.x < -0.35) { held |= p.l1; } if (this.rightStick.value.x > 0.35) { held |= p.r1; }
        out.rightX = 0; out.rightY = 0;
      }
      if (this.down('airFire')) { held |= p.a; } if (this.down('gear')) { held |= p.l3; } if (this.down('towHook')) { held |= p.right; }
      if (this.exitHoldStart >= 0) { this.button('enter').progress = clamp((nowMs - this.exitHoldStart) / 400, 0, 1); if (nowMs - this.exitHoldStart >= 400) { held |= p.y; } }
    } else if (c === 'menu') { out.leftX = this.moveStick.value.x; out.leftY = this.moveStick.value.y; }
    for (const role of this.roles) {
      if (role.kind !== 'characterWheel') { continue; }
      held |= p.down; const d: FiveStarPoint = { x: role.current.x - role.start.x, y: role.current.y - role.start.y }, length: number = magnitude(d);
      out.rightX = length > 24 ? d.x / length : 0; out.rightY = length > 24 ? -d.y / length : 0;
    }
    if (this.down('camera')) { held |= p.select; } if (this.down('pause')) { held |= p.start; }
    if (this.fullMapRequestedAt >= 0) {
      if (c === 'menu' && nowMs - this.fullMapRequestedAt > 1500) { this.pulse(p.a, 120, nowMs); this.fullMapRequestedAt = -1; }
      else if (nowMs - this.fullMapRequestedAt > 5000) { this.fullMapRequestedAt = -1; }
    }
    if (this.down('phone') && this.phoneDock && !this.gamePhoneOut) { held |= p.up; }
    if (!this.down('phone')) { this.phoneOpensDock = false; }
    if (this.down('phone') && this.gamePhoneOut && !this.phoneOpensDock) { held |= p.b; }
    if (this.tvCouch()) {
      if (this.down('left')) { out.leftX = -1; } else if (this.down('right')) { out.leftX = 1; }
      if (this.down('up')) { out.leftY = 1; } else if (this.down('down')) { out.leftY = -1; }
    } else if (this.taxiRide()) {
      if (this.down('up')) { held |= p.up; } if (this.down('down')) { held |= p.down; }
      if (this.down('left')) { out.leftX = -1; } if (this.down('right')) { out.leftX = 1; }
    } else {
      if (this.down('up')) { held |= p.up; } if (this.down('down')) { held |= p.down; }
      if (this.down('left')) { held |= p.left; } if (this.down('right')) { held |= p.right; }
    }
    const common: string[] = ['accept', 'back', 'phone_option', 'taxi_hurry', 'taxi_stop', 'tabLeft', 'tabRight', 'prompt_option'];
    const bits: number[] = [p.a, p.b, p.x, p.x, p.b, p.l1, p.r1, p.x];
    for (let i: number = 0; i < common.length; i++) { if (this.down(common[i])) { held |= bits[i]; } }
    const stickToo: boolean = hasAny(this.polledMission, [220, 221]);
    for (const m of MISSION_BUTTONS) {
      const pressed: boolean = this.down(m.name);
      if (m.trigger !== 0) {
        const asked: boolean = this.polledMission.indexOf(m.input) >= 0 || this.polledMission.indexOf(m.menuInput) >= 0;
        if (stickToo && pressed && this.triggerWasPressed.indexOf(m.input) < 0) {
          if (this.latchedTriggers.indexOf(m.input) >= 0) { this.latchedTriggers = this.latchedTriggers.filter((n: number): boolean => n !== m.input); }
          else { this.latchedTriggers.push(m.input); }
        }
        if (pressed && this.triggerWasPressed.indexOf(m.input) < 0) { this.triggerWasPressed.push(m.input); }
        if (!pressed) { this.triggerWasPressed = this.triggerWasPressed.filter((n: number): boolean => n !== m.input); }
        if (!stickToo || !asked) { this.latchedTriggers = this.latchedTriggers.filter((n: number): boolean => n !== m.input); }
        const latched: boolean = this.latchedTriggers.indexOf(m.input) >= 0;
        this.button(m.name).active = latched;
        if (latched || pressed) { if (m.trigger === 1) { out.leftTrigger = 1; } else { out.rightTrigger = 1; } }
      }
      if (pressed && m.pad !== 0) { held |= m.pad; }
    }
    if (this.down('script_ltrt') && !this.bothTriggersWasPressed) { this.bothTriggersHeld = !this.bothTriggersHeld; }
    this.bothTriggersWasPressed = this.down('script_ltrt');
    if (!(hasAll(this.polledMission, [228, 229]) || hasAll(this.polledMission, [207, 208]))) { this.bothTriggersHeld = false; }
    this.button('script_ltrt').active = this.bothTriggersHeld;
    if (this.bothTriggersHeld) { out.leftTrigger = 1; out.rightTrigger = 1; }
    if (this.phoneDock && !this.gamePhoneOut && ((this.phoneRequestedAt >= 0 && nowMs - this.phoneRequestedAt > 2500) ||
      (this.phoneRequestedAt < 0 && nowMs - this.lastDockInput > 20000))) { this.phoneDock = false; this.phoneRequestedAt = -1; this.refresh(); }
    if (c === 'onFoot' && this.down('detonate')) { held |= p.left; }
    this.pulses = this.pulses.filter((pulse: PadPulse): boolean => pulse.until > nowMs);
    for (const pulse of this.pulses) { held |= pulse.buttons; }
    const raw: number = held;
    for (let bit: number = 0; bit < 16; bit++) {
      const mask: number = 1 << bit;
      let press: PadPress | undefined = this.pressedAt.find((v: PadPress): boolean => v.bit === mask);
      if ((raw & mask) !== 0 && (this.lastHeld & mask) === 0) {
        if (press) { press.at = nowMs; } else { press = { bit: mask, at: nowMs }; this.pressedAt.push(press); }
      }
      if ((raw & mask) === 0 && press && nowMs - press.at < 70) { held |= mask; }
    }
    this.lastHeld = raw; out.buttons = held;
    out.mouseActive = this.mouseLookActive();
    if (out.mouseActive) {
      if (c === 'onFoot') {
        if (out.leftTrigger > 0.5) { out.mouseButtons |= 2; } if (out.rightTrigger > 0.5) { out.mouseButtons |= 1; }
        out.shift = this.moveStick.sprinting || this.down('sprint');
      } else if (fiveStarDriving(c)) {
        if (this.down('driveAim') || this.down('driveBy')) { out.mouseButtons |= 2; }
        if (this.down('driveBy') && this.fireStartedAt >= 0 && nowMs - this.fireStartedAt > 120) { out.mouseButtons |= 1; }
      }
    }
    return out;
  }
  private flushCamera(out: FiveStarOutput): void {
    const mode: number = this.preferences.cameraTouchMode;
    if (mode !== this.cameraMode) { this.clearCameraTravel(); this.cameraMode = mode; }
    if (!this.swipeEnabled() || this.hasSelectionWheel()) { this.clearCameraTravel(); return; }
    const gain: number = mode === 2 ? this.aiming ? this.preferences.mouseAimSensitivity : this.preferences.mouseLookSensitivity :
      this.aiming ? this.preferences.swipeAimSensitivity : this.preferences.swipeLookSensitivity;
    const raw: FiveStarPoint = { x: this.cameraTravel.x * gain, y: this.cameraTravel.y * gain };
    this.cameraTravel = { x: 0, y: 0 };
    if (mode === 1) { out.cameraSwipeX = raw.x; out.cameraSwipeY = -raw.y; }
    else if (mode === 2) {
      const totalX: number = clamp(raw.x + this.fractionalMouse.x, -1600, 1600);
      const totalY: number = clamp(raw.y + this.fractionalMouse.y, -1600, 1600);
      out.mouseDx = Math.trunc(totalX); out.mouseDy = Math.trunc(totalY);
      this.fractionalMouse = { x: totalX - out.mouseDx, y: totalY - out.mouseDy };
    }
  }
  static cameraStick(value: FiveStarPoint, speed: number): FiveStarPoint {
    const length: number = magnitude(value);
    if (length <= 0.08) { return { x: 0, y: 0 }; }
    const x: number = Math.min(1, (length - 0.08) / 0.92 * Math.max(0.3, speed));
    const out: number = 0.24 + (1 - 0.24) * Math.pow(x, 1.4);
    return { x: value.x / length * out, y: value.y / length * out };
  }
  static lookAxis(velocity: number): number {
    const speed: number = Math.abs(velocity);
    return speed <= 4 ? 0 : Math.sign(velocity) * (0.24 + 0.76 * Math.pow(Math.min(1, speed / 320), 0.75));
  }
}
