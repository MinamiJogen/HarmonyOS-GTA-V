import game from 'libharmony_probe.so';
import { LengthMetrics } from '@kit.ArkUI';
import { TouchAction, TouchProfile, DrawerAction, TOUCH_PROFILES, TOUCH_DRAWER_ACTIONS, touchProfile } from './TouchLayout';
import { TouchInputController, TouchInputSink, TouchVector, joystickVector, taskJoystickVector, touchEventPoints, missingTouchUp, TouchMoveTraceGate, clampTouch } from './TouchInput';
import { TouchStateResolver, TouchResolution } from './TouchState';
import { touchSlotFrame, TouchFrame, legalJoystickOrigin, prewarmJoystickGeometry } from './TouchGeometry';
import { TASK_WIDGETS, TaskWidget } from './TaskPadLayout';
import { TouchFamilySettings, TouchLayoutSettings, TouchSlotAdjustment, adjustedTouchFrame, defaultTouchFamilySettings, touchSettingsFamily, validateTouchSettings, sanitizeTouchFamily } from './TouchSettings';
import { readTouchPreferences, writeTouchPreferences } from './TouchPreferenceStore';
import { RawTouchBinding, RawTouchAxis, rawTouchBinding, rawTouchAxis } from './RawTouchBindings';
import { hasLiteralTouchInput, isDirectTouchAction } from './TouchBindingPolicy';

interface ControlPointer { id: number; owner: string; binding: string; x: number; y: number; epoch: number; }
interface GameTouchTraceEntry {
  seq: number; atMs: number; handler: string; phase: string; eventType: number;
  changedCount: number; touchesCount: number; pointId: number; touchX: number; touchY: number;
  stickX: number; stickY: number; stickPointer: number; lookPointer: number;
  axisX: string; axisY: string; vectorX: number; vectorY: number;
  canPlay: boolean; contextEpoch: number;
}
interface GameTouchTrace { seq: number; events: GameTouchTraceEntry[]; lastDown?: GameTouchTraceEntry; }


@Component
export struct GameTouchControls {
  @Prop @Watch('onVisibilityChanged') visible: boolean = true;
  @Prop @Watch('onInputChanged') inputEnabled: boolean = true;
  @Prop @Watch('onSnapshotChanged') stateJson: string = '';
  @Prop @Watch('onResetRequested') resetEpoch: number = 0;
  @Prop @Watch('onCloseRequested') closePanelsEpoch: number = 0;
  @Prop @Watch('onMouseModeChanged') mouseLook: boolean = true;
  @Prop @Watch('onGeometryChanged') gameAspect: number = 0;
  onOpenKeyboard: () => void = (): void => {};
  onPanelChanged: (open: boolean) => void = (_open: boolean): void => {};
  onStatusChanged: (status: string) => void = (_status: string): void => {};
  @State private widthVp: number = 800;
  @State private heightVp: number = 400;
  @State private actions: TouchAction[] = [];
  @State private profileId: string = 'UNKNOWN';
  private status: string = '';
  private statusPublished: boolean = false;
  @State private runActive: boolean = false;
  @State private pressed: string[] = [];
  @State private drawerVisible: boolean = false;
  @State private drawerTab: string = '常用';
  @State private taskVisible: boolean = false;
  @State private manualId: string = '';
  @State private pinnedDetonate: boolean = false;
  @State private stickActive: boolean = false;
  @State private stickX: number = 96;
  @State private stickY: number = 304;
  @State private knobX: number = 0;
  @State private knobY: number = 0;
  @State private taskStickX: number[] = [0, 0];
  @State private taskStickY: number[] = [0, 0];
  @State private taskTriggerValues: number[] = [0, 0];
  @State private feedback: string = '';
  @State private buttonOpacity: number = 55;
  @State private lookSensitivity: number = 1;
  @State private familySettings: TouchFamilySettings = defaultTouchFamilySettings();
  @State private editSlot: string = 'P0';
  @State private editX: number = 0;
  @State private editY: number = 0;
  @State private editDiameter: number = 76;
  private settings: TouchLayoutSettings = { version: 1, families: {} };
  private settingsFamily: string = '';
  private settingsDirectory: string = '';
  private resolver: TouchStateResolver = new TouchStateResolver();
  private controller: TouchInputController = new TouchInputController({
    control: (name: string, value: number, group: number): boolean => game.sendControl(name, value, group),
    rawKey: (name: string, pressed: boolean, source: string): boolean => game.sendRawKey(name, pressed, source),
    mouse: (button: number, pressed: boolean, source: string): boolean => { game.sendMouseButton(button, pressed, source); return true; },
    wheel: (notches: number): void => { game.sendWheel(notches); },
    look: (x: number, y: number, dx: number, dy: number, relative: boolean): void => {
      game.sendPointer(this.getUIContext().vp2px(x), this.getUIContext().vp2px(y), dx, dy, relative);
    }
  } as TouchInputSink);
  private pointers: ControlPointer[] = [];
  private stickPointer: number = -1;
  private lookPointer: number = -1;
  private lookX: number = 0;
  private lookY: number = 0;
  @State private axisX: string = '';
  @State private axisY: string = '';
  @State private directions: string[] = [];
  private snapshotGeneration: number = -1;
  private lastFresh: boolean = false;
  private lastTransport: string = '';
  private timer: number = -1;
  private active: boolean = false;
  private traceMoveGate: TouchMoveTraceGate = new TouchMoveTraceGate();
  private traceFlushScheduled: boolean = false;
  private traceSequence: number = 0;
  private traceEvents: GameTouchTraceEntry[] = [];
  private traceLastDown: GameTouchTraceEntry | undefined = undefined;
  private lastVectorX: number = 0;
  private lastVectorY: number = 0;

  aboutToAppear(): void {
    this.active = true;
    try {
      const context = this.getUIContext().getHostContext();
      if (context) { this.settingsDirectory = context.filesDir; this.settings = readTouchPreferences(this.settingsDirectory); }
    } catch (_) { this.feedback = '已使用默认触控布局'; }
    this.onSnapshotChanged();
    this.timer = setInterval((): void => { if (this.active) { this.refresh(); this.controller.tick(Date.now()); } }, 100);
  }
  aboutToDisappear(): void {
    this.active = false;
    if (this.timer >= 0) { clearInterval(this.timer); this.timer = -1; }
    this.releaseTouches();
    this.onPanelChanged(false);
  }
  private onSnapshotChanged(): void { this.resolver.accept(this.stateJson, Date.now()); this.refresh(); }
  private onResetRequested(): void { this.releaseTouches(); }
  private onCloseRequested(): void { this.closePanels(); }
  private onMouseModeChanged(): void { this.releaseTouches(); }
  private onGeometryChanged(): void { this.releaseTouches(); this.settingsFamily = ''; this.applySettingsFamily(); this.prewarmJoystick(); }
  private onInputChanged(): void { if (!this.inputEnabled) { this.releaseTouches(); } }
  private onVisibilityChanged(): void { if (!this.visible) { this.closePanels(); } }
  private binding(action: TouchAction): string { return `${action.action}|${action.control}|${action.rawKey}|${action.gesture}|${action.uiAction}`; }
  private stateFamily(id: string): string { return id.indexOf('FOOT_') === 0 ? 'FOOT' : id; }
  private refresh(): void {
    this.resolver.manualProfile = this.manualId;
    this.resolver.pinnedDetonate = this.pinnedDetonate;
    const next: TouchResolution = this.resolver.resolve(Date.now());
    const changedGeneration: boolean = this.snapshotGeneration !== next.generation;
    const changedFamily: boolean = this.stateFamily(this.profileId) !== this.stateFamily(next.profile.id);
    const becameStale: boolean = this.lastFresh && !next.fresh;
    const changedTransport: boolean = this.lastTransport.length > 0 && next.transport.length > 0 && this.lastTransport !== next.transport;
    if (changedGeneration || changedFamily || becameStale || changedTransport) { this.releaseTouches(); }
    this.snapshotGeneration = next.generation;
    this.lastFresh = next.fresh; this.lastTransport = next.transport;
    const allowed: string[] = [];
    const displayed: TouchAction[] = [];
    for (let i = 0; i < next.actions.length; i++) {
      let action: TouchAction = next.actions[i];
      if (!isDirectTouchAction(action)) { continue; }
      // A held slot keeps its original action until Up; new touches cannot inherit an old Down.
      if (!changedFamily && !changedGeneration && !becameStale) {
        for (let oldIndex = 0; oldIndex < this.actions.length; oldIndex++) {
          const old: TouchAction = this.actions[oldIndex];
          if (old.slot === action.slot && this.binding(old) !== this.binding(action) && this.controller.held(this.binding(old))) { action = old; }
        }
      }
      displayed.push(action); allowed.push(this.binding(action));
    }
    if (next.profile.id.indexOf('FOOT_') === 0) { allowed.push('raw:runShift'); }
    if (next.axisX.length > 0) { allowed.push(`axis:${next.axisX}`); }
    if (next.axisY.length > 0) { allowed.push(`axis:${next.axisY}`); }
    for (let i = 0; i < next.directions.length; i++) { allowed.push(`axis:${next.directions[i]}`); }
    // Drawer and SCRIPT pad are deliberate raw surfaces, separate from automatic semantic invalidation.
    if (!this.drawerVisible && !this.taskVisible) { this.controller.retainBindings(allowed); }
    if (this.profileId !== next.profile.id) { this.profileId = next.profile.id; }
    this.applySettingsFamily();
    if (!this.statusPublished || this.status !== next.status) {
      this.status = next.status; this.statusPublished = true; this.onStatusChanged(this.status);
    }
    if (this.axisX !== next.axisX) { this.axisX = next.axisX; }
    if (this.axisY !== next.axisY) { this.axisY = next.axisY; }
    if (this.directions.length !== next.directions.length || this.directions.some((direction: string, index: number): boolean => direction !== next.directions[index])) { this.directions = next.directions.slice(); }
    if (!this.sameActions(displayed, this.actions)) { this.actions = displayed; this.prewarmJoystick(); }
    this.updatePressed();
  }
  private prewarmJoystick(): void {
    if (this.axisX.length === 0 && this.axisY.length === 0 && this.directions.length === 0) { return; }
    const slots: string[] = this.actions.map((action: TouchAction): string => action.slot); slots.push('S3');
    if (this.profileId.indexOf('FOOT_') === 0) { slots.push('RUN'); }
    prewarmJoystickGeometry(this.rectWidth(), this.rectHeight(), slots, slots.map((slot: string): TouchFrame => this.frame(slot)));
  }
  private sameActions(left: TouchAction[], right: TouchAction[]): boolean {
    if (left.length !== right.length) { return false; }
    for (let i = 0; i < left.length; i++) {
      if (left[i].slot !== right[i].slot || left[i].label !== right[i].label || this.binding(left[i]) !== this.binding(right[i])) { return false; }
    }
    return true;
  }
  private updatePressed(): void {
    const active: string[] = [];
    for (let i = 0; i < this.actions.length; i++) { const key: string = this.binding(this.actions[i]); if (this.controller.held(key)) { active.push(key); } }
    if (active.length !== this.pressed.length || active.some((binding: string, index: number): boolean => binding !== this.pressed[index])) { this.pressed = active; }
  }
  private releaseTouches(): void {
    this.controller.releaseAll(); game.releaseSemanticInputs(); this.pointers = [];
    this.stickPointer = -1; this.lookPointer = -1; this.lastVectorX = 0; this.lastVectorY = 0;
    this.stickActive = false; this.knobX = 0; this.knobY = 0;
    if (this.pressed.length > 0) { this.pressed = []; }
    if (this.runActive) { this.runActive = false; AppStorage.setOrCreate<boolean>('gameSprintHeld', false); }
    this.taskStickX = [0, 0]; this.taskStickY = [0, 0]; this.taskTriggerValues = [0, 0];
  }
  private closePanels(): void {
    this.releaseTouches(); this.drawerVisible = false; this.taskVisible = false; this.onPanelChanged(false);
  }
  private openDrawer(tab: string = '常用'): void {
    if (!this.inputEnabled) { return; }
    this.releaseTouches(); this.drawerTab = tab; this.taskVisible = false; this.drawerVisible = true; this.onPanelChanged(true);
  }
  private openTask(): void {
    this.releaseTouches(); this.drawerVisible = false; this.taskVisible = true; this.onPanelChanged(true);
  }
  private openKeyboard(): void { this.closePanels(); this.onOpenKeyboard(); }
  private chooseManual(id: string): void { this.releaseTouches(); this.manualId = id; this.closePanels(); this.refresh(); }
  private rectWidth(): number {
    return this.gameAspect > 0 ? Math.min(this.widthVp, this.heightVp * this.gameAspect) : this.widthVp;
  }
  private rectHeight(): number {
    return this.gameAspect > 0 ? Math.min(this.heightVp, this.widthVp / this.gameAspect) : this.heightVp;
  }
  private rectX(): number { return (this.widthVp - this.rectWidth()) / 2; }
  private rectY(): number { return (this.heightVp - this.rectHeight()) / 2; }
  private frame(slot: string): TouchFrame { return adjustedTouchFrame(slot, this.rectWidth(), this.rectHeight(), this.familySettings); }
  private applySettingsFamily(): void {
    const key: string = touchSettingsFamily(this.profileId, this.rectWidth(), this.rectHeight());
    if (key === this.settingsFamily) { return; }
    this.settingsFamily = key;
    let family: TouchFamilySettings = sanitizeTouchFamily(this.settings.families[key]);
    if (Object.keys(family.slots).length > 0 && validateTouchSettings(this.rectWidth(), this.rectHeight(), family).length > 0) {
      family = { opacity: family.opacity, sensitivity: family.sensitivity, slots: {} };
    }
    this.familySettings = family; this.buttonOpacity = family.opacity; this.lookSensitivity = family.sensitivity;
    this.selectEditSlot(this.editSlot);
  }
  private selectEditSlot(slot: string): void {
    this.editSlot = slot;
    const saved: TouchSlotAdjustment | undefined = this.familySettings.slots[slot];
    this.editX = saved ? saved.x : 0; this.editY = saved ? saved.y : 0;
    this.editDiameter = saved ? saved.diameter : touchSlotFrame(slot, this.rectWidth(), this.rectHeight()).diameter;
  }
  private saveFamily(family: TouchFamilySettings): void {
    this.settings.families[this.settingsFamily] = family;
    this.familySettings = family; this.buttonOpacity = family.opacity; this.lookSensitivity = family.sensitivity;
    this.prewarmJoystick();
    try {
      if (this.settingsDirectory.length > 0) { writeTouchPreferences(this.settingsDirectory, this.settings); }
      this.feedback = '已保存当前状态族与屏幕比例的布局';
    } catch (_) { this.feedback = '本次布局已应用，持久保存失败'; }
  }
  private saveLayoutAppearance(): void {
    this.saveFamily({ opacity: this.buttonOpacity, sensitivity: this.lookSensitivity, slots: this.familySettings.slots });
  }
  private saveSlotEdit(): void {
    this.releaseTouches();
    const slots: Record<string, TouchSlotAdjustment> = {};
    for (const key of Object.keys(this.familySettings.slots)) { slots[key] = this.familySettings.slots[key]; }
    slots[this.editSlot] = { x: this.editX, y: this.editY, diameter: this.editDiameter };
    const family: TouchFamilySettings = { opacity: this.buttonOpacity, sensitivity: this.lookSensitivity, slots: slots };
    const problem: string = validateTouchSettings(this.rectWidth(), this.rectHeight(), family);
    if (problem.length > 0) { this.feedback = problem; return; }
    this.saveFamily(family);
  }
  private held(action: TouchAction): boolean { return this.pressed.indexOf(this.binding(action)) >= 0; }
  private canPlay(): boolean { return this.visible && this.inputEnabled && !this.drawerVisible && !this.taskVisible; }
  private findPointer(id: number): ControlPointer | undefined {
    for (let i = 0; i < this.pointers.length; i++) { if (this.pointers[i].id === id) { return this.pointers[i]; } }
    return undefined;
  }
  private removePointer(id: number): void { this.pointers = this.pointers.filter((p: ControlPointer): boolean => p.id !== id); }
  private functionLabel(action: TouchAction): string {
    if (action.uiAction === 'raw_enter') { return '确认'; }
    if (action.uiAction === 'raw_escape') { return '返回'; }
    if (action.uiAction === 'raw_context_e') { return '交互'; }
    return action.label;
  }
  private inputLabel(action: TouchAction): string {
    if (action.rawKey.length > 0) { return action.rawKey.replace('Key', '').replace('Escape', 'Esc'); }
    if (action.uiAction === 'manual_mode_picker') { return '菜单'; }
    if (action.gesture === 'open_keyboard') { return '104 键'; }
    const mapping: RawTouchBinding | undefined = rawTouchBinding(action.control);
    return mapping ? mapping.label : '待验证';
  }
  private pressAction(owner: string, binding: string, action: TouchAction): boolean {
    if (action.rawKey.length > 0) { return this.controller.pressRaw(owner, binding, action.rawKey); }
    const mapping: RawTouchBinding | undefined = rawTouchBinding(action.control);
    return mapping ? this.controller.pressMapped(owner, binding, mapping) : false;
  }
  private rawAxis(owner: string, control: string, value: number): void {
    const axis: RawTouchAxis | undefined = rawTouchAxis(control);
    if (!axis) { return; }
    const negative: RawTouchBinding | undefined = rawTouchBinding(axis.negative);
    const positive: RawTouchBinding | undefined = rawTouchBinding(axis.positive);
    if (value < -0.02 && negative) { this.controller.pressMapped(`${owner}:negative`, `axis:${control}`, negative); }
    else { this.controller.release(`${owner}:negative`); }
    if (value > 0.02 && positive) { this.controller.pressMapped(`${owner}:positive`, `axis:${control}`, positive); }
    else { this.controller.release(`${owner}:positive`); }
  }
  private joystickKeys(): string {
    const x: RawTouchAxis | undefined = rawTouchAxis(this.axisX);
    const y: RawTouchAxis | undefined = rawTouchAxis(this.axisY);
    if (x && y && x.label === 'A / D' && y.label === 'W / S') { return 'W A S D'; }
    return `${x ? x.label : ''}${y ? ` · ${y.label}` : ''}`;
  }
  /** Move carries the active pointer set; some platform/injected events have no changed list. */
  private eventPoints(event: TouchEvent): TouchObject[] {
    return touchEventPoints<TouchObject>(event.type === TouchType.Move, event.changedTouches, event.touches);
  }
  private traceTouch(handler: string, event: TouchEvent, phase: string): void {
    if (event.type === TouchType.Move && !this.traceMoveGate.shouldCapture(event.timestamp, Date.now())) { return; }
    const points: TouchObject[] = this.eventPoints(event);
    const point: TouchObject | undefined = points.length > 0 ? points[0] : undefined;
    this.traceSequence++;
    const entry: GameTouchTraceEntry = {
      seq: this.traceSequence, atMs: Date.now(), handler: handler, phase: phase, eventType: event.type,
      changedCount: event.changedTouches.length, touchesCount: event.touches.length,
      pointId: point ? point.id : -1, touchX: point ? point.x : -1, touchY: point ? point.y : -1,
      stickX: this.stickX, stickY: this.stickY, stickPointer: this.stickPointer, lookPointer: this.lookPointer,
      axisX: this.axisX, axisY: this.axisY, vectorX: this.lastVectorX, vectorY: this.lastVectorY,
      canPlay: this.canPlay(), contextEpoch: this.controller.getEpoch()
    };
    if (event.type === TouchType.Down && phase === 'received') { this.traceLastDown = entry; }
    this.traceEvents.push(entry);
    if (this.traceEvents.length > 16) { this.traceEvents = this.traceEvents.slice(this.traceEvents.length - 16); }
    if (!this.traceFlushScheduled) {
      this.traceFlushScheduled = true;
      Promise.resolve().then((): void => {
        this.traceFlushScheduled = false;
        const trace: GameTouchTrace = { seq: this.traceSequence, events: this.traceEvents, lastDown: this.traceLastDown };
        AppStorage.setOrCreate<string>('gameTouchInputTrace', JSON.stringify(trace));
      });
    }
  }
  private runTouch(event: TouchEvent): void {
    event.stopPropagation();
    if (event.type !== TouchType.Down || !this.canPlay() || this.profileId.indexOf('FOOT_') !== 0) { return; }
    this.traceTouch('run', event, 'received');
    this.runActive = this.controller.toggleRaw('run:toggle', 'raw:runShift', 'ShiftLeft');
    AppStorage.setOrCreate<boolean>('gameSprintHeld', this.runActive);
    game.setInputFocus(true);
    this.traceTouch('run', event, 'processed');
  }
  private actionTouch(action: TouchAction, event: TouchEvent): void {
    this.traceTouch(`action:${action.slot}:${action.label}`, event, 'received');
    event.stopPropagation();
    if (event.type === TouchType.Down && !this.canPlay()) { return; }
    if (event.type === TouchType.Down) { game.setInputFocus(true); }
    const points: TouchObject[] = this.eventPoints(event);
    if (event.type === TouchType.Cancel && points.length === 0) {
      const binding: string = this.binding(action);
      const cancelled: ControlPointer[] = this.pointers.filter((pointer: ControlPointer): boolean => pointer.binding === binding);
      for (let n = 0; n < cancelled.length; n++) { this.controller.release(cancelled[n].owner); this.removePointer(cancelled[n].id); }
    }
    for (let i = 0; i < points.length; i++) {
      const touch: TouchObject = points[i];
      const owner: string = `button:${touch.id}`;
      if (event.type === TouchType.Down) {
        if (this.findPointer(touch.id)) { continue; }
        if (action.uiAction === 'manual_mode_picker') { this.openDrawer('手动模式'); return; }
        if (action.gesture === 'open_keyboard') { this.openKeyboard(); return; }
        if (!hasLiteralTouchInput(action)) { this.feedback = `${action.label}尚待验证，可使用全键盘原始输入`; this.openDrawer('本状态'); return; }
        const binding: string = this.binding(action);
        if (!this.pressAction(owner, binding, action)) { this.feedback = '当前电脑输入通道尚未就绪'; }
        this.pointers.push({ id: touch.id, owner: owner, binding: binding, x: touch.x, y: touch.y, epoch: this.controller.getEpoch() });
      } else {
        const pointer: ControlPointer | undefined = this.findPointer(touch.id);
        if (!pointer || pointer.epoch !== this.controller.getEpoch()) { continue; }
        if (event.type === TouchType.Move && this.mouseLook &&
          (action.gesture === 'hold_drag' || action.gesture === 'hold_drag_release')) {
          this.controller.pointer(0, 0, (touch.x - pointer.x) * this.lookSensitivity,
            (touch.y - pointer.y) * this.lookSensitivity, true);
          pointer.x = touch.x; pointer.y = touch.y;
        }
        if (event.type === TouchType.Up || event.type === TouchType.Cancel) {
          this.controller.release(pointer.owner); this.removePointer(touch.id);
        }
      }
    }
    this.updatePressed();
    this.traceTouch(`action:${action.slot}:${action.label}`, event, 'processed');
  }
  private stickTouch(event: TouchEvent, fromBackground: boolean = false): void {
    this.traceTouch('stick', event, 'received');
    event.stopPropagation();
    if (event.type === TouchType.Cancel) { this.releaseStick(); this.traceTouch('stick', event, 'cancelled'); return; }
    if (missingTouchUp<TouchObject>(event.type === TouchType.Up, this.stickPointer, event.changedTouches, event.touches)) {
      this.releaseStick(); this.traceTouch('stick', event, 'up-missing'); return;
    }
    const points: TouchObject[] = this.eventPoints(event);
    for (let i = 0; i < points.length; i++) {
      const touch: TouchObject = points[i];
      if (event.type === TouchType.Down && this.canPlay() && this.stickPointer < 0) {
        if (fromBackground && touch.x > this.rectWidth() * 0.35) { continue; }
        if (touch.y < this.rectHeight() * 0.3 + 12) { continue; }
        const ids: string[] = this.actions.map((a: TouchAction): string => a.slot); ids.push('S3');
        if (this.profileId.indexOf('FOOT_') === 0) { ids.push('RUN'); }
        const origin: TouchVector = legalJoystickOrigin(touch.x, touch.y, this.rectWidth(), this.rectHeight(), ids, ids.map((id: string): TouchFrame => this.frame(id)));
        this.stickPointer = touch.id; this.stickX = origin.x; this.stickY = origin.y; this.stickActive = true;
        game.setInputFocus(true);
      }
      if (touch.id !== this.stickPointer) { continue; }
      if (event.type === TouchType.Up || event.type === TouchType.Cancel) {
        this.releaseStick(); continue;
      }
      const dx: number = touch.x - this.stickX; const dy: number = touch.y - this.stickY;
      const magnitude: number = Math.max(1, Math.sqrt(dx * dx + dy * dy) / 44);
      this.knobX = dx / magnitude; this.knobY = dy / magnitude;
      const value: TouchVector = joystickVector(dx, dy);
      this.lastVectorX = value.x; this.lastVectorY = value.y;
      if (this.axisX.length > 0) { this.rawAxis('joystick:x', this.axisX, value.x); }
      if (this.axisY.length > 0) { this.rawAxis('joystick:y', this.axisY, value.y); }
      if (this.directions.length === 4) {
        const values: number[] = [Math.max(0, -value.x), Math.max(0, value.x), Math.max(0, -value.y), Math.max(0, value.y)];
        for (let n = 0; n < 4; n++) {
          const mapping: RawTouchBinding | undefined = rawTouchBinding(this.directions[n]);
          if (values[n] > 0.02 && mapping) { this.controller.pressMapped(`joystick:d${n}`, `axis:${this.directions[n]}`, mapping); }
          else { this.controller.release(`joystick:d${n}`); }
        }
      }
    }
    this.traceTouch('stick', event, 'processed');
  }
  private releaseStick(): void {
    this.controller.release('joystick:x'); this.controller.release('joystick:y');
    for (let n = 0; n < 4; n++) { this.controller.release(`joystick:d${n}`); }
    this.stickPointer = -1; this.stickActive = false; this.knobX = 0; this.knobY = 0; this.lastVectorX = 0; this.lastVectorY = 0;
  }
  private lookTouch(event: TouchEvent, backgroundOffsetX: number = 0): void {
    this.traceTouch('look', event, 'received');
    event.stopPropagation();
    if (event.type === TouchType.Cancel) { this.lookPointer = -1; this.traceTouch('look', event, 'cancelled'); return; }
    if (missingTouchUp<TouchObject>(event.type === TouchType.Up, this.lookPointer, event.changedTouches, event.touches)) {
      this.lookPointer = -1; this.traceTouch('look', event, 'up-missing'); return;
    }
    const points: TouchObject[] = this.eventPoints(event);
    for (let i = 0; i < points.length; i++) {
      const touch: TouchObject = points[i];
      const localX: number = touch.x - backgroundOffsetX;
      if (event.type === TouchType.Down && this.canPlay() && this.lookPointer < 0) {
        if (backgroundOffsetX > 0 && localX < 0) { continue; }
        game.setInputFocus(true);
        this.lookPointer = touch.id; this.lookX = localX; this.lookY = touch.y;
        if (!this.mouseLook) { this.controller.pointer(localX + this.rectWidth() * 0.35 + this.rectX(), touch.y + this.rectY(), 0, 0, false); }
      }
      if (touch.id !== this.lookPointer) { continue; }
      if (event.type === TouchType.Move) {
        this.controller.pointer(localX + this.rectWidth() * 0.35 + this.rectX(), touch.y + this.rectY(),
          this.mouseLook ? (localX - this.lookX) * this.lookSensitivity : 0,
          this.mouseLook ? (touch.y - this.lookY) * this.lookSensitivity : 0, this.mouseLook);
        this.lookX = localX; this.lookY = touch.y;
      }
      if (event.type === TouchType.Up || event.type === TouchType.Cancel) { this.lookPointer = -1; }
    }
    this.traceTouch('look', event, 'processed');
  }
  private backgroundTouch(event: TouchEvent): void {
    if (!this.canPlay()) { return; }
    this.traceTouch('background', event, 'received');
    // Child action targets stop propagation; only unused game surface reaches this fallback.
    let leftDown: boolean = false; let rightDown: boolean = false;
    if (event.type === TouchType.Down) {
      const points: TouchObject[] = this.eventPoints(event);
      for (let i = 0; i < points.length; i++) {
        if (points[i].x < this.rectWidth() * 0.35) { leftDown = true; } else { rightDown = true; }
      }
    }
    const routeStick: boolean = this.stickPointer >= 0 || leftDown;
    const routeLook: boolean = this.lookPointer >= 0 || rightDown;
    if (routeStick && (this.axisX.length > 0 || this.axisY.length > 0 || this.directions.length > 0)) { this.stickTouch(event, true); }
    if (routeLook) { this.lookTouch(event, this.rectWidth() * 0.35); }
  }
  private rawTouch(action: DrawerAction, event: TouchEvent): void {
    event.stopPropagation();
    const points: TouchObject[] = this.eventPoints(event);
    for (let i = 0; i < points.length; i++) {
      const t: TouchObject = points[i];
      const owner: string = `drawer:${action.control}:${action.rawKey}:${t.id}`;
      if (event.type === TouchType.Down && this.inputEnabled) {
        if (this.findPointer(t.id)) { continue; }
        game.setInputFocus(true);
        if (action.rawKey.length > 0) { this.controller.pressRaw(owner, owner, action.rawKey); }
        else {
          const mapping: RawTouchBinding | undefined = rawTouchBinding(action.control);
          if (mapping) { this.controller.pressMapped(owner, owner, mapping); }
          else { this.feedback = '此动作没有已确认的电脑按键映射，请使用全键盘'; }
        }
        this.pointers.push({ id: t.id, owner: owner, binding: owner, x: t.x, y: t.y, epoch: this.controller.getEpoch() });
      } else {
        const pointer: ControlPointer | undefined = this.findPointer(t.id);
        if (!pointer || pointer.epoch !== this.controller.getEpoch()) { continue; }
        if (event.type === TouchType.Move && this.mouseLook && (action.control.indexOf('WHEEL') >= 0 || action.control === 'INPUT_SELECT_WEAPON')) {
          this.controller.pointer(0, 0, (t.x - pointer.x) * this.lookSensitivity, (t.y - pointer.y) * this.lookSensitivity, true);
          pointer.x = t.x; pointer.y = t.y;
        }
        if (event.type === TouchType.Up || event.type === TouchType.Cancel) { this.controller.release(owner); this.removePointer(t.id); }
      }
    }
  }
  private drawerActions(): DrawerAction[] {
    const profile: TouchProfile = touchProfile(this.profileId);
    const refs: string[] = this.drawerTab === '常用' ? ['A139', 'A117', 'A002', 'A004'] : profile.drawer;
    // Restricted worlds never grow semantic controls merely by opening a drawer.
    if (this.profileId === 'LOADING' || this.profileId === 'DEAD_FAILURE' || this.profileId === 'BLOCKING_CUTSCENE') {
      return this.profileId === 'BLOCKING_CUTSCENE' ? TOUCH_DRAWER_ACTIONS.filter((a: DrawerAction): boolean => a.action === 'A139') : [];
    }
    return TOUCH_DRAWER_ACTIONS.filter((a: DrawerAction): boolean => refs.indexOf(a.action) >= 0 && (a.rawKey.length > 0 || rawTouchBinding(a.control) !== undefined));
  }
  private pendingActionLabels(): string[] {
    const profile: TouchProfile = touchProfile(this.profileId);
    const labels: string[] = [];
    for (let i = 0; i < profile.slots.length; i++) {
      const action: TouchAction = profile.slots[i];
      if (!isDirectTouchAction(action) && labels.indexOf(action.label) < 0) { labels.push(action.label); }
    }
    for (let i = 0; i < TOUCH_DRAWER_ACTIONS.length; i++) {
      const action: DrawerAction = TOUCH_DRAWER_ACTIONS[i];
      if (profile.drawer.indexOf(action.action) >= 0 && action.rawKey.length === 0 && !rawTouchBinding(action.control) && labels.indexOf(action.label) < 0) { labels.push(action.label); }
    }
    return labels;
  }
  private taskTouch(widget: TaskWidget, event: TouchEvent): void {
    event.stopPropagation();
    const points: TouchObject[] = this.eventPoints(event);
    for (let i = 0; i < points.length; i++) {
      const touch: TouchObject = points[i];
      const owner: string = `script:${widget.id}:${touch.id}`;
      if (event.type === TouchType.Up || event.type === TouchType.Cancel) {
        this.controller.release(`${owner}:x`); this.controller.release(`${owner}:y`); this.controller.release(owner);
        if (widget.kind === 'stick') {
          const index: number = widget.id === 'left_stick' ? 0 : 1;
          const xs: number[] = this.taskStickX.slice(); const ys: number[] = this.taskStickY.slice(); xs[index] = 0; ys[index] = 0;
          this.taskStickX = xs; this.taskStickY = ys;
        }
        continue;
      }
      if (!this.inputEnabled || !this.taskVisible) { continue; }
      // A Move from a previous epoch cannot start a new hold.
      const existing: ControlPointer | undefined = this.findPointer(touch.id);
      if (event.type === TouchType.Down) {
        if (existing) { continue; }
        let occupied: boolean = false;
        for (let n = 0; n < this.pointers.length; n++) { if (this.pointers[n].owner.indexOf(`script:${widget.id}:`) === 0) { occupied = true; } }
        if (occupied) { continue; }
        game.setInputFocus(true);
        this.pointers.push({ id: touch.id, owner: owner, binding: owner, x: touch.x, y: touch.y, epoch: this.controller.getEpoch() });
      } else if (!existing || existing.epoch !== this.controller.getEpoch()) { continue; }
      if (widget.kind === 'stick') {
        const dx: number = touch.x - widget.width / 2; const dy: number = touch.y - widget.height / 2;
        const value: TouchVector = taskJoystickVector(dx, dy);
        this.controller.setControl(`${owner}:x`, owner, widget.channels[0], value.x, 2);
        this.controller.setControl(`${owner}:y`, owner, widget.channels[1], value.y, 2);
        const index: number = widget.id === 'left_stick' ? 0 : 1;
        const xs: number[] = this.taskStickX.slice(); const ys: number[] = this.taskStickY.slice(); xs[index] = value.x * 36; ys[index] = value.y * 36;
        this.taskStickX = xs; this.taskStickY = ys;
      } else {
        const value: number = widget.kind === 'continuous_trigger' ? clampTouch(1 - touch.y / widget.height, 0, 1) : 1;
        if (!this.controller.setControl(owner, owner, widget.channels[0], value, 2)) { this.feedback = 'SCRIPT 输入通道暂不可用，请用全键盘'; }
      }
    }
    if (event.type === TouchType.Up || event.type === TouchType.Cancel) {
      for (let i = 0; i < event.changedTouches.length; i++) { this.removePointer(event.changedTouches[i].id); }
    }
  }

  @Builder
  private actionButton(action: TouchAction) {
    Column({ space: 4 }) {
      Text(this.functionLabel(action)).fontSize(action.slot === 'P0' ? 16 : 14).minFontSize(10).maxFontSize(action.slot === 'P0' ? 16 : 14)
        .fontWeight(FontWeight.Bold).fontColor('#FFFFFF').width(this.frame(action.slot).diameter - 8)
        .maxLines(1).textAlign(TextAlign.Center).constraintSize({ maxHeight: 24 })
      Text(this.inputLabel(action)).fontSize(9).minFontSize(8).maxFontSize(9).fontWeight(FontWeight.Medium).fontColor('#C7DCE8').width(this.frame(action.slot).diameter - 8).maxLines(1).textAlign(TextAlign.Center)
    }
    .justifyContent(FlexAlign.Center).width(this.frame(action.slot).diameter).height(this.frame(action.slot).diameter)
    .backgroundColor(this.held(action) ? '#D95B859A' : '#6B152733')
    .backgroundBlurStyle(BlurStyle.Thin, { colorMode: ThemeColorMode.DARK })
    .borderRadius(this.frame(action.slot).diameter / 2)
    .border({ width: this.held(action) ? 1.6 : 0.8, color: this.held(action) ? '#E8D6F5FF' : '#70DEEBF2' })
    .shadow({ radius: this.held(action) ? 13 : 5, color: this.held(action) ? '#705ACCE8' : '#36000000', offsetY: 2 })
    .opacity(this.held(action) ? 1 : this.buttonOpacity / 70)
    .position({ x: this.frame(action.slot).x - this.frame(action.slot).diameter / 2, y: this.frame(action.slot).y - this.frame(action.slot).diameter / 2 })
    .accessibilityText(`${action.label}，${this.inputLabel(action)}`)
    .hitTestBehavior(HitTestMode.Block)
    .onTouch((event: TouchEvent): void => { this.actionTouch(action, event); })
    .onMouse((event: MouseEvent): void => { event.stopPropagation(); })
  }

  @Builder
  private moreButton() {
    Column({ space: 4 }) {
      Text('更多').fontSize(14).minFontSize(10).maxFontSize(14).fontWeight(FontWeight.Bold).fontColor('#FFFFFF').width(this.frame('S3').diameter - 8).maxLines(1).textAlign(TextAlign.Center)
      Text('菜单').fontSize(9).fontColor('#C7DCE8')
    }.justifyContent(FlexAlign.Center).width(this.frame('S3').diameter).height(this.frame('S3').diameter)
      .backgroundColor('#701A2B38').backgroundBlurStyle(BlurStyle.Thin, { colorMode: ThemeColorMode.DARK })
      .borderRadius(30).border({ width: 0.8, color: '#70DEEBF2' })
      .position({ x: this.frame('S3').x - this.frame('S3').diameter / 2, y: this.frame('S3').y - this.frame('S3').diameter / 2 })
      .accessibilityText('更多动作').onClick((): void => { this.openDrawer(); })
      .onTouch((event: TouchEvent): void => { event.stopPropagation(); })
      .onMouse((event: MouseEvent): void => { event.stopPropagation(); })
  }

  @Builder
  private runButton() {
    Column({ space: 2 }) {
      Text('奔跑').fontSize(14).fontWeight(FontWeight.Bold).fontColor(this.runActive ? '#FFFFFF' : '#E7F2FA')
      Text('Shift').fontSize(8).fontColor(this.runActive ? '#DCFDEB' : '#ABC5D5')
    }.justifyContent(FlexAlign.Center).width(44).height(44).borderRadius(22)
      .backgroundColor(this.runActive ? '#D34B8B70' : '#72152733')
      .border({ width: this.runActive ? 1.6 : 0.8, color: this.runActive ? '#C6FFE3' : '#70DEEBF2' })
      .position({ x: this.frame('RUN').x - 22, y: this.frame('RUN').y - 22 })
      .accessibilityText(this.runActive ? '奔跑已开启，Shift，点击关闭' : '奔跑已关闭，Shift，点击开启')
      .accessibilitySelected(this.runActive)
      .hitTestBehavior(HitTestMode.Block)
      .onTouch((event: TouchEvent): void => { this.runTouch(event); })
      .onMouse((event: MouseEvent): void => { event.stopPropagation(); })
  }

  @Builder
  private stickVisual() {
    Stack({ alignContent: Alignment.Center }) {
      Circle().width(128).height(128).fill(this.stickActive ? '#38182C3C' : '#20182C3C').stroke('#65DEEBF2').strokeWidth(0.8)
      Circle().width(86).height(86).fill(Color.Transparent).stroke('#2EDEEBF2').strokeWidth(0.6)
      Circle().width(54).height(54).fill(this.stickActive ? '#AA7598A9' : '#586A8290').stroke('#B5E3F3FF').strokeWidth(1)
        .translate({ x: this.knobX, y: this.knobY }).shadow({ radius: 9, color: '#30000000', offsetY: 2 })
      Text(this.profileId.indexOf('VEHICLE_') === 0 ? '操纵' : '移动').fontSize(14).fontWeight(FontWeight.Bold).fontColor('#EAF4FC').translate({ y: -45 })
      Text(this.joystickKeys()).fontSize(9).fontColor('#BBD0DD').translate({ y: 46 })
    }.width(128).height(128).position({ x: (this.stickActive ? this.stickX : 96) - 64, y: (this.stickActive ? this.stickY : this.rectHeight() - 96) - 64 })
      .hitTestBehavior(HitTestMode.None)
  }

  @Builder
  private rawActionButton(action: DrawerAction) {
    Column({ space: 5 }) {
      Text(action.label).fontSize(14).fontWeight(FontWeight.Bold).fontColor('#FFFFFF').maxLines(2).textAlign(TextAlign.Center)
      Text(action.rawKey.length > 0 ? action.rawKey.replace('Key', '').replace('Escape', 'Esc') : rawTouchBinding(action.control)?.label || '待验证').fontSize(10).fontColor('#AFCBDB')
    }.justifyContent(FlexAlign.Center).width(126).height(76)
      .backgroundColor('#601D3444').borderRadius(18).border({ width: 0.7, color: '#40597487' })
      .onTouch((event: TouchEvent): void => { this.rawTouch(action, event); })
  }

  @Builder
  private drawer() {
    Column({ space: 12 }) {
      Row({ space: 10 }) {
        Column({ space: 3 }) {
          Text('更多动作').fontSize(20).fontWeight(FontWeight.Bold).fontColor('#F5FAFF')
        }.alignItems(HorizontalAlign.Start)
        Blank()
        Button('全键盘').height(44).fontSize(12).fontColor('#C6E7F7').backgroundColor('#253B4B').onClick((): void => { this.openKeyboard(); })
        Button('任务手柄').height(44).fontSize(12).fontColor('#C6E7F7').backgroundColor('#253B4B').onClick((): void => { this.openTask(); })
        Button('×').width(44).height(44).fontSize(23).fontColor('#CBE0EB').backgroundColor('#203544').accessibilityText('关闭更多动作').onClick((): void => { this.closePanels(); })
      }.width('100%')
      Row({ space: 6 }) {
        ForEach(['常用', '本状态', '特殊', '手动模式', '布局'], (tab: string): void => {
          Button(tab).height(44).fontSize(12).fontColor(this.drawerTab === tab ? '#EAF9FF' : '#8EAABA')
            .backgroundColor(this.drawerTab === tab ? '#3B6378' : '#172A38').borderRadius(14)
            .onClick((): void => { this.releaseTouches(); this.drawerTab = tab; })
        })
      }.width('100%')
      Scroll() {
        Column({ space: 12 }) {
          if (this.drawerTab === '手动模式') {
            Text('手动模式只切换布局，不修改游戏世界。原始映射与任务语义仍待真机验证。').fontSize(12).fontColor('#9EB6C7').width('100%')
            Button('自动识别').height(44).backgroundColor(this.manualId.length === 0 ? '#3B6378' : '#253B4B').fontColor('#EAF9FF').onClick((): void => { this.chooseManual(''); })
            Flex({ wrap: FlexWrap.Wrap, space: { main: LengthMetrics.vp(8), cross: LengthMetrics.vp(8) } }) {
              ForEach(TOUCH_PROFILES.filter((p: TouchProfile): boolean => p.id !== 'UNKNOWN' && p.id !== 'LOADING' && p.id !== 'DEAD_FAILURE' && p.id !== 'BLOCKING_CUTSCENE' && p.id !== 'GAME_STARTUP'), (profile: TouchProfile): void => {
                Button(profile.label).height(44).fontSize(12).fontColor('#E4F2F8').backgroundColor(this.manualId === profile.id ? '#3B6378' : '#203543')
                  .onClick((): void => { this.chooseManual(profile.id); })
              }, (profile: TouchProfile): string => profile.id)
            }.width('100%')
          } else if (this.drawerTab === '布局') {
            Text('横屏手机 / 展开折叠屏 · 保持按钮尺寸与安全边距').fontSize(12).fontColor('#A9C3D2').width('100%')
            Row({ space: 12 }) { Text('按键透明度').fontSize(13).fontColor('#EAF9FF'); Slider({ value: this.buttonOpacity, min: 35, max: 70, step: 1 }).layoutWeight(1).onChange((value: number): void => { this.buttonOpacity = value; }); }.width('100%')
            Row({ space: 12 }) { Text('视角灵敏度').fontSize(13).fontColor('#EAF9FF'); Slider({ value: this.lookSensitivity, min: 0.3, max: 2, step: 0.1 }).layoutWeight(1).onChange((value: number): void => { this.lookSensitivity = value; }); }.width('100%')
            Button('保存透明度与灵敏度').height(44).fontColor('#DFF3FF').backgroundColor('#294456').onClick((): void => { this.saveLayoutAppearance(); })
            Text('调整按键位置与尺寸 · 按状态族和宽屏 / 折叠屏分别保存').fontSize(12).fontColor('#A9C3D2').width('100%')
            Flex({ wrap: FlexWrap.Wrap, space: { main: LengthMetrics.vp(6), cross: LengthMetrics.vp(6) } }) {
              ForEach(['P0', 'P1', 'P2', 'S0', 'S1', 'S2', 'S3', 'S4', 'S5', 'T0', 'T1', 'T2', 'T3'], (slot: string): void => {
                Button(slot).height(44).width(50).fontSize(12).fontColor('#DFF3FF').backgroundColor(this.editSlot === slot ? '#3B6378' : '#203543')
                  .onClick((): void => { this.selectEditSlot(slot); })
              })
            }.width('100%')
            Row({ space: 10 }) { Text(`水平偏移 ${this.editX} vp`).width(124).fontSize(12).fontColor('#EAF9FF'); Slider({ value: this.editX, min: -160, max: 160, step: 4 }).layoutWeight(1).onChange((value: number): void => { this.editX = value; }); }.width('100%')
            Row({ space: 10 }) { Text(`垂直偏移 ${this.editY} vp`).width(124).fontSize(12).fontColor('#EAF9FF'); Slider({ value: this.editY, min: -160, max: 160, step: 4 }).layoutWeight(1).onChange((value: number): void => { this.editY = value; }); }.width('100%')
            Row({ space: 10 }) { Text(`直径 ${this.editDiameter} vp`).width(124).fontSize(12).fontColor('#EAF9FF'); Slider({ value: this.editDiameter, min: 44, max: 104, step: 2 }).layoutWeight(1).onChange((value: number): void => { this.editDiameter = value; }); }.width('100%')
            Row({ space: 8 }) {
              Button('保存此按键').height(44).fontColor('#DFF3FF').backgroundColor('#355E74').onClick((): void => { this.saveSlotEdit(); })
              Button('恢复默认布局').height(44).fontColor('#DFF3FF').backgroundColor('#294456').onClick((): void => { this.releaseTouches(); this.saveFamily(defaultTouchFamilySettings()); this.selectEditSlot(this.editSlot); })
            }
            Text('保存前检测 44 vp 最小触点、安全边界、小地图、摇杆与预留任务槽，重叠时保留原布局。').fontSize(11).fontColor('#91A9B9').width('100%')
          } else if (this.drawerTab === '特殊') {
            Row({ space: 12 }) {
              Text('固定显示引爆按钮').fontSize(13).fontColor('#EAF9FF')
              Blank()
              Toggle({ type: ToggleType.Switch, isOn: this.pinnedDetonate }).onChange((value: boolean): void => { this.releaseTouches(); this.pinnedDetonate = value; this.refresh(); })
            }.width('100%').height(48)
            Text('手动固定仅提供原始引爆通道，不判断已放置炸弹或当前可执行性。').fontSize(12).fontColor('#9EB6C7').width('100%')
            this.rawActionButton({ action: 'raw_e', label: '原始交互 E', control: '', rawKey: 'KeyE', icon: 'E' })
            this.rawActionButton({ action: 'raw_f', label: '原始上车 F', control: '', rawKey: 'KeyF', icon: 'F' })
          } else {
            Flex({ wrap: FlexWrap.Wrap, space: { main: LengthMetrics.vp(8), cross: LengthMetrics.vp(8) } }) {
              ForEach(this.drawerActions(), (action: DrawerAction): void => { this.rawActionButton(action); }, (action: DrawerAction): string => action.control)
              if (this.drawerTab === '本状态') {
                this.rawActionButton({ action: 'raw_e', label: '原始 E', control: '', rawKey: 'KeyE', icon: 'E' })
                this.rawActionButton({ action: 'raw_enter', label: '原始 Enter', control: '', rawKey: 'Enter', icon: '↵' })
                this.rawActionButton({ action: 'raw_escape', label: '原始 Esc', control: '', rawKey: 'Escape', icon: '↩' })
              }
            }.width('100%')
            if (this.drawerTab === '本状态' && this.pendingActionLabels().length > 0) {
              Text('尚未指定电脑按键').fontSize(12).fontWeight(FontWeight.Medium).fontColor('#AFC4D0').width('100%')
              ForEach(this.pendingActionLabels(), (label: string): void => {
                Text(`${label} · 未绑定`).fontSize(12).fontColor('#8097A6').width('100%')
              }, (label: string): string => label)
            }
            Text('大字为功能，小字为电脑按键。按下发送键鼠 Down，松手或取消发送 Up；未绑定项只列在目录中。').fontSize(11).fontColor('#91A9B9').width('100%')
          }
          if (this.feedback.length > 0) { Text(this.feedback).fontSize(12).fontColor('#FFD7A2').width('100%'); }
        }.width('100%').alignItems(HorizontalAlign.Start)
      }.layoutWeight(1).scrollBar(BarState.Auto).width('100%')
    }.padding(20).width(Math.min(700, this.widthVp - 28)).height(Math.min(410, this.heightVp - 28))
      .backgroundColor('#F012202D').backgroundBlurStyle(BlurStyle.Thick, { colorMode: ThemeColorMode.DARK })
      .borderRadius(28).border({ width: 1, color: '#507894A5' }).shadow({ radius: 28, color: '#60000000', offsetY: 10 })
      .onTouch((event: TouchEvent): void => { event.stopPropagation(); })
      .onMouse((event: MouseEvent): void => { event.stopPropagation(); })
  }

  @Builder
  private taskWidget(widget: TaskWidget) {
    Stack({ alignContent: Alignment.Center }) {
      if (widget.kind === 'stick') {
        Circle().width(128).height(128).fill('#6B203B4C').stroke('#608AAABF').strokeWidth(1)
        Circle().width(54).height(54).fill('#A65C8198').stroke('#BCE3F5FF').strokeWidth(1)
          .translate({ x: this.taskStickX[widget.id === 'left_stick' ? 0 : 1], y: this.taskStickY[widget.id === 'left_stick' ? 0 : 1] })
      }
      Text(widget.label).fontSize(widget.kind === 'stick' ? 16 : 13).fontWeight(FontWeight.Medium).fontColor('#E9F6FF')
    }.width(widget.width).height(widget.height)
      .backgroundColor(widget.kind === 'stick' ? Color.Transparent : '#83426378')
      .borderRadius(widget.kind === 'continuous_trigger' ? 14 : widget.width / 2)
      .border({ width: widget.kind === 'stick' ? 0 : 1, color: '#739DB7C9' })
      .position({ x: widget.x, y: widget.y })
      .accessibilityText(`原始 SCRIPT 通道 ${widget.label}`)
      .onTouch((event: TouchEvent): void => { this.taskTouch(widget, event); })
  }

  @Builder
  private taskPad() {
    if (this.widthVp >= 640 && this.heightVp >= 360) {
      Stack({ alignContent: Alignment.TopStart }) {
        ForEach(TASK_WIDGETS, (widget: TaskWidget): void => { this.taskWidget(widget); }, (widget: TaskWidget): string => widget.id)
        Column({ space: 5 }) {
          Text('SCRIPT 任务手柄').fontSize(17).fontWeight(FontWeight.Bold).fontColor('#ECF8FF')
          Text('19 个原始通道 · 任务语义待验证').fontSize(10).fontColor('#9DB8C8')
          Button('全键盘').height(44).fontSize(12).fontColor('#CFECFB').backgroundColor('#29485B').onClick((): void => { this.openKeyboard(); })
        }.width(192).position({ x: 224, y: 106 })
        Button('×').width(44).height(44).fontSize(22).fontColor('#D8EDF9').backgroundColor('#29485B')
          .position({ x: 584, y: 10 }).accessibilityText('关闭任务手柄').onClick((): void => { this.closePanels(); })
      }.width(640).height(360).backgroundColor('#EE132533')
        .backgroundBlurStyle(BlurStyle.Thick, { colorMode: ThemeColorMode.DARK })
        .borderRadius(24).border({ width: 1, color: '#607799AB' })
        .onTouch((event: TouchEvent): void => { event.stopPropagation(); })
        .onMouse((event: MouseEvent): void => { event.stopPropagation(); })
    } else {
      Column({ space: 16 }) {
        Text('当前可用区域不足 640 × 360 vp').fontSize(17).fontColor('#EFF8FF')
        Text('任务手柄在此屏幕规格尚未验收，请使用全键盘原始输入。').fontSize(13).fontColor('#A8C4D6').textAlign(TextAlign.Center)
        Row({ space: 12 }) {
          Button('全键盘').height(44).onClick((): void => { this.openKeyboard(); })
          Button('关闭').height(44).onClick((): void => { this.closePanels(); })
        }
      }.padding(24).width(Math.min(460, this.widthVp - 28)).backgroundColor('#F0142837').borderRadius(24)
    }
  }

  build() {
    Stack({ alignContent: Alignment.Center }) {
      if (this.visible) {
        Stack({ alignContent: Alignment.TopStart }) {
          if (!this.drawerVisible && !this.taskVisible && this.inputEnabled) {
            // Blank right area exclusively owns its touch; control nodes above it block penetration.
            Rect().width(this.rectWidth() * 0.65).height(this.rectHeight()).fill(Color.Transparent)
              .position({ x: this.rectWidth() * 0.35, y: 0 }).hitTestBehavior(HitTestMode.Block)
              .onTouch((event: TouchEvent): void => { this.lookTouch(event); })
            if (this.axisX.length > 0 || this.axisY.length > 0 || this.directions.length > 0) {
              Rect().width(this.rectWidth() * 0.35).height(this.rectHeight()).fill(Color.Transparent)
                .hitTestBehavior(HitTestMode.Block).onTouch((event: TouchEvent): void => { this.stickTouch(event); })
              this.stickVisual()
            }
            ForEach(this.actions, (action: TouchAction): void => { this.actionButton(action); }, (action: TouchAction): string => `${action.slot}:${this.binding(action)}`)
            if (this.profileId !== 'LOADING') { this.moreButton(); }
            if (this.profileId.indexOf('FOOT_') === 0) { this.runButton(); }
          }
        }.width(this.rectWidth()).height(this.rectHeight()).position({ x: this.rectX(), y: this.rectY() }).hitTestBehavior(HitTestMode.Transparent)
          .onTouch((event: TouchEvent): void => { this.backgroundTouch(event); })
        if (this.drawerVisible || this.taskVisible) {
          Column().width('100%').height('100%').backgroundColor('#35060E16')
            .onTouch((event: TouchEvent): void => { event.stopPropagation(); })
            .onMouse((event: MouseEvent): void => { event.stopPropagation(); })
          if (this.drawerVisible) { this.drawer(); }
          if (this.taskVisible) { this.taskPad(); }
        }
      }
    }.width('100%').height('100%').hitTestBehavior(HitTestMode.Transparent)
      .onAreaChange((_old: Area, next: Area): void => {
        const width: number = Number(next.width); const height: number = Number(next.height);
        if (Math.abs(width - this.widthVp) > 1 || Math.abs(height - this.heightVp) > 1) {
          this.releaseTouches(); this.widthVp = width; this.heightVp = height; this.settingsFamily = ''; this.applySettingsFamily(); this.prewarmJoystick();
        }
      })
  }
}
