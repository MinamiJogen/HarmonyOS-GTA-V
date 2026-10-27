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
  lookAxis?: string;
  lookStep?: number;
}
export const TOUCH_MOUSE_REPEAT_MS: number = 100;
export function clampTouch(value: number, minimum: number, maximum: number): number {
  return Math.max(minimum, Math.min(maximum, value));
}
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
export interface TouchButtonContact { id: number; target: string; owner: string; epoch: number; startedAt: number; }

/** Button contacts begin only on Down. Multiple fingers may hold one target;
 * ending one contact never releases another still-active contact or button. */
export class TouchButtonTracker {
  private contacts: TouchButtonContact[] = [];
  contains(id: number): boolean { return this.contacts.some((c: TouchButtonContact): boolean => c.id === id); }
  begin(target: string, id: number, epoch: number, timestamp: number = 0): TouchButtonContact | undefined {
    if (this.contains(id)) { return undefined; }
    const contact: TouchButtonContact = { id: id, target: target, owner: `button:${target}:${id}`,
      epoch: epoch, startedAt: timestamp };
    this.contacts.push(contact);
    return contact;
  }
  current(target: string, id: number, epoch: number): TouchButtonContact | undefined {
    return this.contacts.find((c: TouchButtonContact): boolean => c.target === target && c.id === id && c.epoch === epoch);
  }
  end(target: string, ending: boolean, cancelled: boolean, changed: TouchPointIdentity[],
    active: TouchPointIdentity[], timestamp: number = Number.POSITIVE_INFINITY): TouchButtonContact[] {
    if (!ending && !cancelled) { return []; }
    const ended: TouchButtonContact[] = this.contacts.filter((c: TouchButtonContact): boolean => {
      if (c.target !== target || timestamp < c.startedAt) { return false; }
      if (cancelled) { return true; }
      return changed.some((p: TouchPointIdentity): boolean => p.id === c.id) ||
        !active.some((p: TouchPointIdentity): boolean => p.id === c.id);
    });
    this.contacts = this.contacts.filter((c: TouchButtonContact): boolean => ended.indexOf(c) < 0);
    return ended;
  }
  reset(): void { this.contacts = []; }
}

export class TouchInputController {
  private sink: TouchInputSink;
  private owners: TouchInputOwner[] = [];
  private epoch: number = 0;
  private serial: number = 0;
  private nextLookAt: number = 0;
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
  pressLookDirection(id: string, binding: string, axis: string, sign: number,
    step: number = 6, wallMs: number = Date.now()): boolean {
    if ((axis !== 'x' && axis !== 'y') || (sign !== -1 && sign !== 1) || !Number.isFinite(step) || step <= 0) { return false; }
    if (this.existing(id) >= 0) { return true; }
    this.owners.push({ id: id, binding: binding, control: '', rawKey: '', mouseButton: -1,
      wheel: 0, nextWheelAt: 0, value: sign, group: 0, lookAxis: axis, lookStep: Math.min(24, step) });
    this.repeatLookDirections(wallMs);
    return true;
  }
  private repeatLookDirections(wallMs: number): void {
    if (wallMs < this.nextLookAt) { return; }
    let negativeX: boolean = false; let positiveX: boolean = false;
    let negativeY: boolean = false; let positiveY: boolean = false;
    let maxX: number = 0; let maxY: number = 0;
    for (const owner of this.owners) {
      const step: number = owner.lookStep || 0;
      if (owner.lookAxis === 'x') {
        if (owner.value < 0) { negativeX = true; } else { positiveX = true; }
        maxX = Math.max(maxX, step);
      }
      if (owner.lookAxis === 'y') {
        if (owner.value < 0) { negativeY = true; } else { positiveY = true; }
        maxY = Math.max(maxY, step);
      }
    }
    if (maxX === 0 && maxY === 0) { return; }
    // Opposite directions cancel. Multiple fingers cannot multiply movement,
    // and a delayed UI tick emits only one bounded step, never a catch-up burst.
    this.nextLookAt = wallMs + TOUCH_MOUSE_REPEAT_MS;
    const x: number = ((positiveX ? 1 : 0) - (negativeX ? 1 : 0)) * maxX;
    const y: number = ((positiveY ? 1 : 0) - (negativeY ? 1 : 0)) * maxY;
    if (x !== 0 || y !== 0) { this.sink.look(0, 0, x, y, true); }
  }
  tick(wallMs: number): void {
    for (let i = 0; i < this.owners.length; i++) {
      const owner: TouchInputOwner = this.owners[i];
      if (owner.wheel !== 0 && wallMs >= owner.nextWheelAt) { this.sink.wheel(owner.wheel); owner.nextWheelAt = wallMs + 120; }
    }
    this.repeatLookDirections(wallMs);
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
    if (!this.owners.some((owner: TouchInputOwner): boolean => owner.lookAxis !== undefined)) { this.nextLookAt = 0; }
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
