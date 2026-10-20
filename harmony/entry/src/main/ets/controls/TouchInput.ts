import { RawTouchBinding } from './RawTouchBindings';
/** Input transport is injectable so ownership and transition policy can be tested without a device. */
export interface TouchInputSink {
  control: (name: string, value: number, group: number) => boolean;
  rawKey: (name: string, pressed: boolean, source: string) => boolean;
  mouse: (button: number, pressed: boolean, source: string) => boolean;
  wheel: (notches: number) => void;
  look: (x: number, y: number, dx: number, dy: number, relative: boolean) => void;
}
interface TouchInputOwner {
  id: string;
  binding: string;
  control: string;
  rawKey: string;
  mouseButton: number;
  wheel: number;
  nextWheelAt: number;
  value: number;
  group: number;
}
export function clampTouch(value: number, minimum: number, maximum: number): number {
  return Math.max(minimum, Math.min(maximum, value));
}
export interface TouchVector { x: number; y: number; }
/** Active points are authoritative during Move; edge events identify changed contacts. */
export function touchEventPoints<T>(moving: boolean, changed: T[], active: T[]): T[] {
  if (moving && active.length > 0) { return active; }
  return changed.length > 0 ? changed : active;
}
export interface TouchPointIdentity { id: number; }
/** A missing ending contact must not leave an axis held, or release a different active finger. */
export function missingTouchUp<T extends TouchPointIdentity>(ending: boolean, tracked: number, changed: T[], active: T[]): boolean {
  if (!ending || tracked < 0) { return false; }
  for (let i = 0; i < changed.length; i++) { if (changed[i].id === tracked) { return false; } }
  for (let i = 0; i < active.length; i++) { if (active[i].id === tracked) { return false; } }
  return true;
}
/** All handler phases of one timestamp share one decision, while Move batches remain <= 10 Hz. */
export class TouchMoveTraceGate {
  private lastSampleMs: number = Number.NEGATIVE_INFINITY;
  private eventTimestamp: number = Number.NaN;
  private capture: boolean = false;
  shouldCapture(timestamp: number, wallMs: number): boolean {
    if (timestamp === this.eventTimestamp) { return this.capture; }
    this.eventTimestamp = timestamp;
    this.capture = wallMs - this.lastSampleMs >= 100;
    if (this.capture) { this.lastSampleMs = wallMs; }
    return this.capture;
  }
}
export function joystickVector(dx: number, dy: number, radius: number = 64): TouchVector {
  const magnitude: number = Math.sqrt(dx * dx + dy * dy) / radius;
  if (magnitude <= 0.18) { return { x: 0, y: 0 }; }
  const strength: number = Math.pow((Math.min(1, magnitude) - 0.18) / 0.82, 1.35);
  return { x: dx / (magnitude * radius) * strength, y: dy / (magnitude * radius) * strength };
}

export function taskJoystickVector(dx: number, dy: number, radius: number = 64): TouchVector {
  const magnitude: number = Math.sqrt(dx * dx + dy * dy) / radius;
  if (magnitude <= 0.12) { return { x: 0, y: 0 }; }
  const strength: number = (Math.min(1, magnitude) - 0.12) / 0.88;
  return { x: dx / (magnitude * radius) * strength, y: dy / (magnitude * radius) * strength };
}

export class TouchInputController {
  private sink: TouchInputSink;
  private owners: TouchInputOwner[] = [];
  private epoch: number = 0;
  private serial: number = 0;
  constructor(sink: TouchInputSink) { this.sink = sink; }
  getEpoch(): number { return this.epoch; }
  private existing(id: string): number {
    for (let i = 0; i < this.owners.length; i++) { if (this.owners[i].id === id) { return i; } }
    return -1;
  }
  private aggregate(control: string, group: number): number {
    let value: number = 0;
    for (let i = 0; i < this.owners.length; i++) {
      const owner: TouchInputOwner = this.owners[i];
      if (owner.control === control && owner.group === group) { value += owner.value; }
    }
    return clampTouch(value, -1, 1);
  }
  held(binding: string): boolean {
    for (let i = 0; i < this.owners.length; i++) { if (this.owners[i].binding === binding && this.owners[i].value !== 0) { return true; } }
    return false;
  }
  setControl(id: string, binding: string, name: string, value: number, group: number = 0): boolean {
    if (name.length === 0 || !Number.isFinite(value)) { return false; }
    const index: number = this.existing(id);
    if (index >= 0 && (this.owners[index].control !== name || this.owners[index].group !== group)) { this.release(id); }
    const current: number = this.existing(id);
    const owner: TouchInputOwner = { id: id, binding: binding, control: name, rawKey: '', mouseButton: -1, wheel: 0, nextWheelAt: 0, value: clampTouch(value, -1, 1), group: group };
    if (current >= 0) { this.owners[current] = owner; } else { this.owners.push(owner); }
    const accepted: boolean = this.sink.control(name, this.aggregate(name, group), group);
    if (!accepted) { this.owners = this.owners.filter((entry: TouchInputOwner) => entry.id !== id); }
    return accepted;
  }
  pressRaw(id: string, binding: string, key: string): boolean {
    if (this.existing(id) >= 0) { return true; } // auto-repeat never increments an owner count.
    if (!this.sink.rawKey(key, true, `touch:${this.epoch}:${id}`)) { return false; }
    this.owners.push({ id: id, binding: binding, control: '', rawKey: key, mouseButton: -1, wheel: 0, nextWheelAt: 0, value: 1, group: 0 });
    return true;
  }
  toggleRaw(id: string, binding: string, key: string): boolean {
    if (this.existing(id) >= 0) { this.release(id); return false; }
    return this.pressRaw(id, binding, key);
  }
  pressMouse(id: string, binding: string, button: number): boolean {
    if (this.existing(id) >= 0) { return true; }
    if (!this.sink.mouse(button, true, `touch:${this.epoch}:${id}`)) { return false; }
    this.owners.push({ id: id, binding: binding, control: '', rawKey: '', mouseButton: button, wheel: 0, nextWheelAt: 0, value: 1, group: 0 });
    return true;
  }
  pressMapped(id: string, binding: string, mapping: RawTouchBinding, wallMs: number = Date.now()): boolean {
    let accepted: boolean = true;
    for (let i = 0; i < mapping.keys.length; i++) { accepted = this.pressRaw(`${id}:key:${i}`, binding, mapping.keys[i]) && accepted; }
    for (let i = 0; i < mapping.mouseButtons.length; i++) { accepted = this.pressMouse(`${id}:mouse:${i}`, binding, mapping.mouseButtons[i]) && accepted; }
    if (mapping.wheel !== 0 && this.existing(`${id}:wheel`) < 0) {
      this.sink.wheel(mapping.wheel);
      this.owners.push({ id: `${id}:wheel`, binding: binding, control: '', rawKey: '', mouseButton: -1, wheel: mapping.wheel,
        nextWheelAt: wallMs + 320, value: 1, group: 0 });
    }
    if (!accepted) { this.release(id); }
    return accepted;
  }
  tick(wallMs: number): void {
    for (let i = 0; i < this.owners.length; i++) {
      const owner: TouchInputOwner = this.owners[i];
      if (owner.wheel !== 0 && wallMs >= owner.nextWheelAt) { this.sink.wheel(owner.wheel); owner.nextWheelAt = wallMs + 120; }
    }
  }
  release(id: string): void {
    const ids: string[] = this.owners.filter((owner: TouchInputOwner): boolean => owner.id === id || owner.id.indexOf(`${id}:`) === 0).map((owner: TouchInputOwner): string => owner.id);
    for (let i = 0; i < ids.length; i++) {
      const index: number = this.existing(ids[i]);
      if (index < 0) { continue; }
      const old: TouchInputOwner = this.owners[index]; this.owners.splice(index, 1);
      if (old.rawKey.length > 0) { this.sink.rawKey(old.rawKey, false, `touch:${this.epoch}:${old.id}`); }
      if (old.mouseButton >= 0) { this.sink.mouse(old.mouseButton, false, `touch:${this.epoch}:${old.id}`); }
      if (old.control.length > 0) { this.sink.control(old.control, this.aggregate(old.control, old.group), old.group); }
    }
  }
  retainBindings(bindings: string[]): void {
    const releaseIds: string[] = [];
    for (let i = 0; i < this.owners.length; i++) {
      if (bindings.indexOf(this.owners[i].binding) < 0) { releaseIds.push(this.owners[i].id); }
    }
    for (let i = 0; i < releaseIds.length; i++) { this.release(releaseIds[i]); }
  }
  releaseAll(): void {
    const ids: string[] = this.owners.map((owner: TouchInputOwner) => owner.id);
    for (let i = 0; i < ids.length; i++) { this.release(ids[i]); }
    this.epoch++;
  }
  /** For accessibility click activation only; touchscreen buttons keep Down/Up, not a short pulse. */
  nextOwner(prefix: string): string { this.serial++; return `${prefix}:${this.serial}`; }
  pointer(x: number, y: number, dx: number, dy: number, relative: boolean): void {
    this.sink.look(x, y, dx, dy, relative);
  }
}
