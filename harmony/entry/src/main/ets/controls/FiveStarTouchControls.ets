import game from 'libharmony_probe.so';
import { systemDateTime } from '@kit.BasicServicesKit';
import { LengthMetrics } from '@kit.ArkUI';
import { vibrator } from '@kit.SensorServiceKit';
import { FiveStarInput, FiveStarOutput, FiveStarSnapshot, defaultFiveStarSnapshot, parseFiveStarSnapshot } from './FiveStarInput';
import { FiveStarButton, FiveStarStickDisplay, FiveStarInsets, FiveStarPoint, FiveStarRect, FiveStarContext } from './FiveStarLayout';
import { FiveStarSettings, FiveStarLayoutEntry, defaultFiveStarSettings, sanitizeFiveStarSettings } from './FiveStarSettings';
import { readFiveStarPreferences, writeFiveStarPreferences } from './FiveStarPreferenceStore';
import { FiveStarMotion } from './FiveStarMotion';

/**
 * ArkUI view/storage bridge for the complete Five Star touch input model.
 * Copyright (c) 2026 GTA V iOS port contributors. Original source MIT;
 * see upstream/fivestar-input-8e2e4628/LICENSE, commit 8e2e4628.
 * HarmonyOS adaptation distributed under GPL-3.0-or-later.
 * Glyphs here use platform text/geometry; no game or upstream ControlIcons assets.
 */
interface FiveStarHostSnapshot extends FiveStarSnapshot {
  known: boolean;
  hookReady: boolean;
  ageMs: number;
  generation: number;
  modsActive?: boolean;
  minimapRect?: FiveStarRect;
}
interface FiveStarEditorTouch { id: number; x: number; y: number; }
interface FiveStarModContact { id: number; key: string; }

const ESSENTIAL_CONTROLS: string[] = ['fire', 'aim', 'enter', 'gas', 'brake', 'pause', 'settings', 'airFire'];
const MOD_KEYS: string[] = ['F1', 'F2', 'F3', 'F4', 'F5', 'F6', 'F7', 'F8', 'F9', 'F10',
  'ArrowUp', 'ArrowLeft', 'ArrowDown', 'ArrowRight', 'Enter', 'Backspace', 'Numpad8', 'Numpad4', 'Numpad5', 'Numpad6', 'Numpad2', 'Escape'];
const CONTROL_GLYPHS: Record<string, string> = {
  'fire': '◎', 'aim': '⊕', 'leftFire': '◎', 'jump': '⇧', 'sprint': '跑', 'enter': '⇥', 'weapons': '⊙', 'reload': '↻',
  'cover': '盾', 'crouch': '蹲', 'switchCharacter': '切', 'special': 'ϟ', 'interact': '手', 'detonate': '✺',
  'zoomIn': '＋', 'zoomOut': '−', 'weaponPrev': '‹', 'weaponNext': '›', 'gas': '↑', 'brake': '↓', 'handbrake': 'Ⓟ',
  'steer_left': '◀', 'steer_right': '▶', 'driveBy': '◎', 'driveAim': '⊕', 'duck': '↓', 'horn': '喇叭', 'lookBack': '↶',
  'vehicleWeapon': '换武', 'towHook': '吊钩', 'towArmUp': '↟', 'towArmDown': '↡', 'roof': '车顶', 'detachTrailer': '脱钩',
  'dive': '⇣', 'surface': '⇡', 'airFire': '◎', 'gear': '起落', 'throttleUp': '⇈', 'throttleDown': '⇊', 'yawLeft': '↶', 'yawRight': '↷',
  'camera': '▣', 'phone': '▯', 'pause': 'Ⅱ', 'settings': '⚙', 'mods': 'MOD', 'lights': '灯', 'radio': '♫',
  'up': '▲', 'down': '▼', 'left': '◀', 'right': '▶', 'accept': '✓', 'back': '↩', 'phone_option': '⋮',
  'taxi_hurry': '快', 'taxi_stop': '停', 'prompt_option': '跳过', 'tabLeft': '≪', 'tabRight': '≫',
  'script_ltrt': 'LT+RT', 'script_lb': 'LB', 'script_rb': 'RB', 'script_lt': 'LT', 'script_rt': 'RT',
  'script_x': 'X', 'script_y': 'Y', 'script_a': 'A', 'script_b': 'B', 'script_ls': 'L3', 'script_rs': 'R3'
};

@Component
export struct FiveStarTouchControls {
  @Prop @Watch('onVisibilityChanged') visible: boolean = true;
  @Prop @Watch('onInputChanged') inputEnabled: boolean = true;
  @Prop @Watch('onSnapshotChanged') stateJson: string = '';
  @Prop @Watch('onResetRequested') resetEpoch: number = 0;
  @Prop @Watch('onCloseRequested') closePanelsEpoch: number = 0;
  @Prop @Watch('onOpenControlsSettings') controlsSettingsEpoch: number = 0;
  @Prop @Watch('onGeometryChanged') gameAspect: number = 0;
  @Prop @Watch('onGeometryChanged') safeLeft: number = 0;
  @Prop @Watch('onGeometryChanged') safeRight: number = 0;
  @Prop @Watch('onGeometryChanged') safeTop: number = 0;
  @Prop @Watch('onGeometryChanged') safeBottom: number = 0;
  @Prop @Watch('onControllerChanged') controllerConnected: boolean = false;
  @Prop @Watch('onResetRequested') mouseLook: boolean = true;
  onOpenKeyboard: () => void = (): void => {};
  onOpenSettings: () => void = (): void => {};
  onPanelChanged: (open: boolean) => void = (_open: boolean): void => {};
  onStatusChanged: (status: string) => void = (_status: string): void => {};
  onSettingsChanged: (settingsJson: string) => void = (_json: string): void => {};

  @State private widthVp: number = 800;
  @State private heightVp: number = 400;
  @State private buttons: FiveStarButton[] = [];
  @State private stickDisplays: FiveStarStickDisplay[] = [];
  @State private preferences: FiveStarSettings = defaultFiveStarSettings();
  @State private panel: string = '';
  @State private editing: boolean = false;
  @State private editorContext: string = 'onFoot';
  @State private selected: string = '';
  @State private selectedScale: number = 1;
  @State private selectedHidden: boolean = false;
  @State private feedback: string = '';
  @State private contextName: string = 'onFoot';
  @State private ready: boolean = false;
  private engine: FiveStarInput = new FiveStarInput();
  private motion: FiveStarMotion = new FiveStarMotion();
  private snapshot: FiveStarSnapshot = defaultFiveStarSnapshot();
  private hookReady: boolean = false;
  private snapshotAge: number = Number.POSITIVE_INFINITY;
  private snapshotReceivedAt: number = 0;
  private snapshotGeneration: number = -1;
  private lastSampleAt: number = 0;
  private lastVisualJson: string = '';
  private statusText: string = '';
  private timer: number = -1;
  private saveTimer: number = -1;
  private filesDirectory: string = '';
  private active: boolean = false;
  private mouseHeld: number = 0;
  private mouseModeActive: boolean = false;
  private mouseModeKnown: boolean = false;
  private shiftHeld: boolean = false;
  private contacts: number[] = [];
  private editorTouches: FiveStarEditorTouch[] = [];
  private dragOffset: FiveStarPoint = { x: 0, y: 0 };
  private pinchDistance: number = 0;
  private pinchScale: number = 1;
  private modContacts: FiveStarModContact[] = [];
  private mapPinchDistance: number = 0;

  aboutToAppear(): void {
    this.active = true;
    try {
      const context = this.getUIContext().getHostContext();
      if (context) {
        this.filesDirectory = context.filesDir;
        const saved: string = readFiveStarPreferences(this.filesDirectory);
        if (saved.length > 0) { this.preferences = sanitizeFiveStarSettings(JSON.parse(saved) as FiveStarSettings); }
      }
    } catch (_) { this.feedback = '已载入默认触控设置'; }
    this.engine.setSettings(this.preferences);
    this.publishPreferences();
    this.updateViewport();
    this.onSnapshotChanged();
    this.lastSampleAt = this.now();
    this.timer = setInterval((): void => { this.tick(); }, 16);
  }

  aboutToDisappear(): void {
    this.active = false;
    if (this.timer >= 0) { clearInterval(this.timer); this.timer = -1; }
    if (this.saveTimer >= 0) { clearTimeout(this.saveTimer); this.saveTimer = -1; this.saveNow(); }
    this.releaseTouches();
    this.onPanelChanged(false);
  }

  private now(): number { return systemDateTime.getUptime(systemDateTime.TimeType.STARTUP, false); }
  private onVisibilityChanged(): void { if (!this.visible) { this.closePanels(); } }
  private onInputChanged(): void { if (!this.inputEnabled) { this.releaseTouches(); } }
  private onResetRequested(): void { this.releaseTouches(); }
  private onCloseRequested(): void { this.closePanels(); }
  private onControllerChanged(): void { this.releaseTouches(); this.refreshVisuals(); }
  private onGeometryChanged(): void { this.releaseTouches(); this.updateViewport(); this.refreshVisuals(); }
  private onOpenControlsSettings(): void {
    if (!this.active || !this.visible) { return; }
    this.releaseTouches(); this.editing = false; this.engine.editing = false;
    this.panel = 'settings'; this.onPanelChanged(true); this.refreshVisuals();
  }

  private onSnapshotChanged(): void {
    const now: number = this.now();
    try {
      const raw: FiveStarHostSnapshot = JSON.parse(this.stateJson) as FiveStarHostSnapshot;
      const generation: number = Number.isFinite(raw.generation) ? raw.generation : -1;
      if (generation !== this.snapshotGeneration) { this.releaseTouches(); this.snapshotGeneration = generation; }
      this.hookReady = raw.hookReady === true;
      this.snapshotAge = Number.isFinite(raw.ageMs) ? Math.max(0, raw.ageMs) : Number.POSITIVE_INFINITY;
      this.snapshotReceivedAt = now;
      this.snapshot = parseFiveStarSnapshot(this.stateJson);
      this.engine.modsActive = raw.modsActive === true;
      if (raw.minimapRect && raw.minimapRect.width > 0 && raw.minimapRect.height > 0) {
        this.engine.setMinimapRect({ x: raw.minimapRect.x * this.rectWidth(), y: raw.minimapRect.y * this.rectHeight(),
          width: raw.minimapRect.width * this.rectWidth(), height: raw.minimapRect.height * this.rectHeight() });
      } else { this.engine.setMinimapRect(); }
      this.engine.update(this.snapshot, now);
    } catch (_) {
      this.hookReady = false; this.snapshotAge = Number.POSITIVE_INFINITY; this.releaseTouches();
    }
    this.refreshVisuals();
  }

  private isFresh(): boolean { return this.hookReady && this.snapshotAge + this.now() - this.snapshotReceivedAt <= 1500; }
  private hiddenForController(): boolean { return this.controllerConnected && this.preferences.hideTouchWithController; }
  private canPlay(): boolean {
    return this.active && this.visible && this.inputEnabled && this.panel.length === 0 && !this.editing && this.isFresh() && !this.hiddenForController();
  }
  private rectWidth(): number { return this.gameAspect > 0 ? Math.min(this.widthVp, this.heightVp * this.gameAspect) : this.widthVp; }
  private rectHeight(): number { return this.gameAspect > 0 ? Math.min(this.heightVp, this.widthVp / this.gameAspect) : this.heightVp; }
  private rectX(): number { return (this.widthVp - this.rectWidth()) / 2; }
  private rectY(): number { return (this.heightVp - this.rectHeight()) / 2; }
  private viewportInsets(): FiveStarInsets {
    return { left: Math.max(0, this.safeLeft - this.rectX()), right: Math.max(0, this.safeRight - this.rectX()),
      top: Math.max(0, this.safeTop - this.rectY()), bottom: Math.max(0, this.safeBottom - this.rectY()) };
  }
  private updateViewport(): void { this.engine.setViewport(this.rectWidth(), this.rectHeight(), this.viewportInsets()); }

  private tick(): void {
    if (!this.active) { return; }
    const now: number = this.now();
    const elapsed: number = Math.min(0.05, Math.max(0, (now - this.lastSampleAt) / 1000));
    this.lastSampleAt = now;
    const wasReady: boolean = this.ready;
    this.ready = this.isFresh();
    if (wasReady && !this.ready) { this.releaseTouches(); }
    if (this.canPlay()) {
      const output: FiveStarOutput = this.engine.sample(now, elapsed);
      this.sendOutput(output, elapsed);
    }
    this.refreshVisuals();
  }

  private sendOutput(output: FiveStarOutput, elapsed: number = 0): void {
    this.motion.apply(output, this.engine.context, this.engine.aiming, this.preferences, elapsed,
      this.canPlay() && output.active, this.engine.flyingWithRightStick);
    if (!this.mouseModeKnown || this.mouseModeActive !== output.mouseActive) {
      game.setVirtualMouseMode(output.mouseActive); this.mouseModeActive = output.mouseActive; this.mouseModeKnown = true;
    }
    game.sendVirtualPad(output.buttons, output.leftX, output.leftY, output.rightX, output.rightY,
      output.leftTrigger, output.rightTrigger, output.active);
    game.setVirtualRadio(output.radioX, output.radioY, output.radioActive);
    if (output.mouseDx !== 0 || output.mouseDy !== 0) { game.sendMouseDelta(output.mouseDx, output.mouseDy); }
    if (output.cameraSwipeX !== 0 || output.cameraSwipeY !== 0) { game.queueVirtualCameraSwipe(output.cameraSwipeX, output.cameraSwipeY); }
    for (const bit of [1, 2]) {
      if ((output.mouseButtons & bit) !== (this.mouseHeld & bit)) { game.sendMouseButton(bit === 1 ? 0 : 2, (output.mouseButtons & bit) !== 0, 'fivestar-touch'); }
    }
    this.mouseHeld = output.mouseButtons;
    if (output.shift !== this.shiftHeld) { game.sendRawKey('ShiftLeft', output.shift, 'fivestar-touch'); this.shiftHeld = output.shift; }
    if (output.mouseWheel !== 0) { game.sendWheel(output.mouseWheel); }
    if (output.nextAircraftWeapon) { game.nextVirtualAircraftWeapon(); }
    if (output.haptic) {
      try { vibrator.startVibration({ type: 'time', duration: 8 }, { usage: 'touch' }).catch((): void => {}); } catch (_) {}
    }
    if (output.openSettings) { this.releaseTouches(); this.onOpenSettings(); }
    if (output.openMods) { this.releaseTouches(); this.panel = 'mods'; this.onPanelChanged(true); }
  }

  private releaseTouches(): void {
    this.motion.stop();
    this.engine.reset(); this.contacts = []; this.editorTouches = []; this.pinchDistance = 0;
    this.mapPinchDistance = 0;
    game.releaseVirtualPad(); game.setVirtualRadio(0, 0, false);
    game.setVirtualMouseMode(false); this.mouseModeActive = false; this.mouseModeKnown = true;
    if ((this.mouseHeld & 1) !== 0) { game.sendMouseButton(0, false, 'fivestar-touch'); }
    if ((this.mouseHeld & 2) !== 0) { game.sendMouseButton(2, false, 'fivestar-touch'); }
    if (this.shiftHeld) { game.sendRawKey('ShiftLeft', false, 'fivestar-touch'); }
    this.mouseHeld = 0; this.shiftHeld = false;
    for (const contact of this.modContacts) { game.sendRawKey(contact.key, false, 'fivestar-mods'); }
    this.modContacts = [];
  }

  private refreshVisuals(): void {
    const next: FiveStarButton[] = this.engine.visibleButtons();
    const sticks: FiveStarStickDisplay[] = this.engine.sticks();
    const json: string = JSON.stringify(next) + JSON.stringify(sticks);
    if (json !== this.lastVisualJson) {
      // The pure input engine mutates its models. ArkUI V1 must receive detached display values.
      this.buttons = JSON.parse(JSON.stringify(next)) as FiveStarButton[];
      this.stickDisplays = JSON.parse(JSON.stringify(sticks)) as FiveStarStickDisplay[];
      this.lastVisualJson = json;
    }
    this.contextName = this.engine.context;
    const status: string = !this.isFresh() ? '等待游戏输入通道' : this.contextLabel(this.contextName);
    if (status !== this.statusText) { this.statusText = status; this.onStatusChanged(status); }
  }

  private surfaceTouch(event: TouchEvent): void {
    event.stopPropagation();
    if (this.editing) { this.editTouch(event); return; }
    const now: number = this.now();
    if (event.type === TouchType.Cancel) { this.releaseTouches(); this.refreshVisuals(); return; }
    const points: TouchObject[] = event.type === TouchType.Move && event.touches.length > 0 ? event.touches :
      (event.changedTouches.length > 0 ? event.changedTouches : event.touches);
    if (event.type === TouchType.Down && this.canPlay()) {
      game.setInputFocus(true);
      this.mouseModeKnown = false;
      for (const touch of points) {
        if (this.contacts.indexOf(touch.id) >= 0) { continue; }
        this.contacts.push(touch.id); this.engine.touchDown(touch.id, touch.x, touch.y, now);
      }
      if (this.engine.context === 'menu' && event.touches.length >= 2) {
        this.mapPinchDistance = this.touchDistance(event.touches[0], event.touches[1]); this.engine.beginMapPinch();
      }
    } else if (event.type === TouchType.Move && this.canPlay()) {
      for (const touch of points) { if (this.contacts.indexOf(touch.id) >= 0) { this.engine.touchMove(touch.id, touch.x, touch.y, now); } }
      if (this.engine.context === 'menu' && this.mapPinchDistance > 0 && event.touches.length >= 2) {
        this.engine.mapPinch(this.touchDistance(event.touches[0], event.touches[1]) / this.mapPinchDistance);
      }
    } else if (event.type === TouchType.Up) {
      const ended: number[] = this.contacts.filter((id: number): boolean =>
        event.changedTouches.some((touch: TouchObject): boolean => touch.id === id) ||
        !event.touches.some((touch: TouchObject): boolean => touch.id === id));
      for (const id of ended) { this.engine.touchUp(id, now); }
      this.contacts = this.contacts.filter((id: number): boolean => ended.indexOf(id) < 0);
      if (event.touches.length < 2) { this.mapPinchDistance = 0; }
    }
    // Send Down/Up immediately; the timer drives only held axes, repeat and pulse expiry.
    if (this.canPlay()) { this.sendOutput(this.engine.sample(now, 0)); }
    this.refreshVisuals();
  }
  private touchDistance(first: TouchObject, second: TouchObject): number {
    const dx: number = first.x - second.x; const dy: number = first.y - second.y;
    return Math.max(1, Math.sqrt(dx * dx + dy * dy));
  }

  private closePanels(): void {
    this.releaseTouches(); this.panel = ''; this.editing = false; this.engine.editing = false;
    this.engine.update(this.snapshot, this.now()); this.onPanelChanged(false); this.refreshVisuals();
  }
  private publishPreferences(): void {
    const json: string = JSON.stringify(this.preferences);
    this.onSettingsChanged(json); game.setVirtualTargetingMode(this.preferences.targetingMode);
  }
  private applyPreferences(): void {
    this.preferences = sanitizeFiveStarSettings(this.preferences);
    this.engine.setSettings(this.preferences); this.publishPreferences(); this.refreshVisuals();
    if (this.saveTimer >= 0) { clearTimeout(this.saveTimer); }
    this.saveTimer = setTimeout((): void => { this.saveTimer = -1; this.saveNow(); }, 300);
  }
  private saveNow(): void {
    if (this.filesDirectory.length === 0) { return; }
    try { writeFiveStarPreferences(this.filesDirectory, JSON.stringify(this.preferences)); this.feedback = '触控设置已保存'; }
    catch (_) { this.feedback = '设置已应用，保存失败'; }
  }
  private setNumber(field: string, value: number): void {
    this.releaseTouches();
    switch (field) {
      case 'controlOpacity': this.preferences.controlOpacity = value; break;
      case 'controlScale': this.preferences.controlScale = value; break;
      case 'cameraTouchMode': this.preferences.cameraTouchMode = value; break;
      case 'lookSensitivity': this.preferences.lookSensitivity = value; break;
      case 'aimSensitivity': this.preferences.aimSensitivity = value; break;
      case 'mouseLookSensitivity': this.preferences.mouseLookSensitivity = value; break;
      case 'mouseAimSensitivity': this.preferences.mouseAimSensitivity = value; break;
      case 'targetingMode': this.preferences.targetingMode = value; break;
      case 'gyroAim': this.preferences.gyroAim = value; break;
      case 'gyroSensitivity': this.preferences.gyroSensitivity = value; break;
      case 'tiltSteer': this.preferences.tiltSteer = value; break;
      case 'tiltFullLock': this.preferences.tiltFullLock = value; break;
    }
    this.applyPreferences();
  }
  private setFlag(field: string, value: boolean): void {
    this.releaseTouches();
    switch (field) {
      case 'fireAlsoAims': this.preferences.fireAlsoAims = value; break;
      case 'sprintButton': this.preferences.sprintButton = value; break;
      case 'steeringButtons': this.preferences.steeringButtons = value; break;
      case 'airplaneRightStick': this.preferences.airplaneRightStick = value; break;
      case 'hideTouchWithController': this.preferences.hideTouchWithController = value; break;
      case 'gyroInvertY': this.preferences.gyroInvertY = value; break;
    }
    this.applyPreferences();
  }

  private beginEditing(): void {
    this.releaseTouches(); this.panel = ''; this.editing = true; this.engine.editing = true;
    this.chooseEditorContext('onFoot'); this.onPanelChanged(true);
  }
  private chooseEditorContext(context: string): void {
    this.editorContext = context; this.selected = ''; this.editorTouches = [];
    this.engine.setEditorContext(context as FiveStarContext); this.refreshVisuals();
  }
  private selectControl(button: FiveStarButton): void {
    this.selected = button.name;
    const entry: FiveStarLayoutEntry | undefined = this.engine.getLayoutEntry(button.name);
    this.selectedScale = entry ? entry.scale : 1; this.selectedHidden = entry ? entry.hidden : false;
  }
  private selectedButton(): FiveStarButton | undefined { return this.buttons.find((button: FiveStarButton): boolean => button.name === this.selected); }
  private storeSelected(center?: FiveStarPoint, scale?: number, hidden?: boolean): void {
    const button: FiveStarButton | undefined = this.selectedButton();
    if (!button) { return; }
    const normalized: FiveStarPoint = this.engine.normalized(center || { x: button.frame.x + button.frame.width / 2, y: button.frame.y + button.frame.height / 2 });
    const entry: FiveStarLayoutEntry = { x: normalized.x, y: normalized.y,
      scale: scale === undefined ? this.selectedScale : Math.max(0.6, Math.min(1.8, scale)),
      hidden: hidden === undefined ? this.selectedHidden : hidden };
    this.engine.setLayoutEntry(this.selected, entry);
    this.preferences = this.engine.getSettings(); this.selectedScale = entry.scale; this.selectedHidden = entry.hidden;
    this.publishPreferences(); this.refreshVisuals();
    if (this.saveTimer >= 0) { clearTimeout(this.saveTimer); }
    this.saveTimer = setTimeout((): void => { this.saveTimer = -1; this.saveNow(); }, 300);
  }
  private editTouch(event: TouchEvent): void {
    if (event.type === TouchType.Cancel) { this.editorTouches = []; this.pinchDistance = 0; return; }
    if (event.type === TouchType.Down) {
      const changed: TouchObject[] = event.changedTouches.length > 0 ? event.changedTouches : event.touches;
      for (const touch of changed) {
        if (this.editorTouches.some((entry: FiveStarEditorTouch): boolean => entry.id === touch.id)) { continue; }
        if (this.editorTouches.length === 0) {
          const button: FiveStarButton | undefined = this.buttons.slice().reverse().find((entry: FiveStarButton): boolean =>
            touch.x >= entry.frame.x - 8 && touch.x <= entry.frame.x + entry.frame.width + 8 &&
            touch.y >= entry.frame.y - 8 && touch.y <= entry.frame.y + entry.frame.height + 8);
          if (button) {
            this.selectControl(button); this.dragOffset = { x: button.frame.x + button.frame.width / 2 - touch.x, y: button.frame.y + button.frame.height / 2 - touch.y };
          } else { this.selected = ''; }
        }
        this.editorTouches.push({ id: touch.id, x: touch.x, y: touch.y });
      }
      if (this.editorTouches.length >= 2) {
        this.pinchDistance = this.editorDistance(); this.pinchScale = this.selectedScale;
      }
    } else if (event.type === TouchType.Move) {
      for (const touch of event.touches) {
        const point: FiveStarEditorTouch | undefined = this.editorTouches.find((entry: FiveStarEditorTouch): boolean => entry.id === touch.id);
        if (point) { point.x = touch.x; point.y = touch.y; }
      }
      if (this.selected.length > 0 && this.editorTouches.length >= 2 && this.pinchDistance > 0) {
        this.storeSelected(undefined, this.pinchScale * this.editorDistance() / this.pinchDistance);
      } else if (this.selected.length > 0 && this.editorTouches.length === 1) {
        const touch: FiveStarEditorTouch = this.editorTouches[0];
        this.storeSelected(this.engine.clampedToSafeArea({ x: touch.x + this.dragOffset.x, y: touch.y + this.dragOffset.y }));
      }
    } else if (event.type === TouchType.Up) {
      this.editorTouches = this.editorTouches.filter((entry: FiveStarEditorTouch): boolean =>
        event.touches.some((touch: TouchObject): boolean => touch.id === entry.id) &&
        !event.changedTouches.some((touch: TouchObject): boolean => touch.id === entry.id));
      this.pinchDistance = 0;
    }
  }
  private editorDistance(): number {
    if (this.editorTouches.length < 2) { return 0; }
    const dx: number = this.editorTouches[0].x - this.editorTouches[1].x;
    const dy: number = this.editorTouches[0].y - this.editorTouches[1].y;
    return Math.sqrt(dx * dx + dy * dy);
  }
  private finishEditing(): void {
    this.preferences = this.engine.getSettings(); this.editing = false; this.engine.editing = false;
    this.saveNow(); this.panel = 'settings'; this.engine.update(this.snapshot, this.now()); this.refreshVisuals();
  }

  private modTouch(key: string, event: TouchEvent): void {
    event.stopPropagation();
    if (event.type === TouchType.Down && this.panel === 'mods' && this.inputEnabled) {
      game.setInputFocus(true);
      game.setVirtualMouseMode(false); this.mouseModeActive = false; this.mouseModeKnown = true;
      for (const touch of event.changedTouches.length > 0 ? event.changedTouches : event.touches) {
        if (this.modContacts.some((contact: FiveStarModContact): boolean => contact.id === touch.id)) { continue; }
        this.modContacts.push({ id: touch.id, key: key }); game.sendRawKey(key, true, 'fivestar-mods');
      }
    }
    if (event.type === TouchType.Up || event.type === TouchType.Cancel) {
      const ended: FiveStarModContact[] = this.modContacts.filter((contact: FiveStarModContact): boolean => contact.key === key &&
        (event.type === TouchType.Cancel || event.changedTouches.some((touch: TouchObject): boolean => touch.id === contact.id) ||
          !event.touches.some((touch: TouchObject): boolean => touch.id === contact.id)));
      this.modContacts = this.modContacts.filter((contact: FiveStarModContact): boolean => ended.indexOf(contact) < 0);
      if (ended.length > 0 && !this.modContacts.some((contact: FiveStarModContact): boolean => contact.key === key)) { game.sendRawKey(key, false, 'fivestar-mods'); }
    }
  }
  private glyph(button: FiveStarButton): string { return button.icon.indexOf('text:') === 0 ? button.icon.substring(5) : CONTROL_GLYPHS[button.name] || button.label.substring(0, 4); }
  private contextLabel(context: string): string {
    const names: Record<string, string> = { 'onFoot': '步行', 'car': '车辆', 'bike': '摩托车', 'bicycle': '自行车', 'boat': '船', 'sub': '潜艇', 'heli': '直升机', 'plane': '飞机', 'menu': '菜单' };
    return names[context] || context;
  }
  private keyLabel(key: string): string { return key.replace('ArrowUp', '↑').replace('ArrowDown', '↓').replace('ArrowLeft', '←').replace('ArrowRight', '→').replace('Numpad', 'Num ').replace('Backspace', '退格').replace('Escape', 'Esc'); }

  @Builder
  private controlButton(button: FiveStarButton) {
    Stack() {
      Text(this.glyph(button)).fontSize(Math.max(13, Math.min(31, Math.min(button.frame.width, button.frame.height) * 0.38)))
        .fontWeight(FontWeight.Bold).fontColor(button.pressed ? '#E6000000' : '#EBFFFFFF')
        .textAlign(TextAlign.Center).maxLines(1).width('88%')
      if (button.progress > 0) {
        Progress({ value: button.progress * 100, total: 100, type: ProgressType.Ring }).width(button.frame.width).height(button.frame.height)
          .color('#E6FFFFFF').style({ strokeWidth: 3 })
      }
    }.width(button.frame.width).height(button.frame.height)
      .backgroundColor(button.pressed ? '#D9FFFFFF' : '#4D000000')
      .borderRadius(button.pedal ? 18 : button.frame.width / 2)
      .border({ width: this.editing && this.selected === button.name ? 3 : button.active ? 2.5 : 1,
        color: this.editing && this.selected === button.name ? '#FFD56E' : button.active ? '#5DB6E5' : '#48FFFFFF' })
      .shadow({ radius: 6, color: '#40000000' })
      .opacity(this.editing && button.hidden ? 0.28 : button.opacity)
      .position({ x: button.frame.x, y: button.frame.y })
      .accessibilityText(button.label).accessibilitySelected(button.active)
      .id(`fivestar-control-${button.name}`).hitTestBehavior(HitTestMode.None)
  }

  @Builder
  private floatingStick(stick: FiveStarStickDisplay) {
    if (stick.visible) {
      Stack() {
        Circle().width(stick.diameter).height(stick.diameter).fill('#1F000000').stroke(stick.sprinting ? '#D95DB6E5' : '#4DFFFFFF').strokeWidth(stick.sprinting ? 3 : 2)
        Circle().width(stick.diameter * 0.45).height(stick.diameter * 0.45).fill('#73FFFFFF')
          .position({ x: stick.diameter * 0.275 + stick.valueX * stick.travel, y: stick.diameter * 0.275 - stick.valueY * stick.travel })
      }.width(stick.diameter).height(stick.diameter).position({ x: stick.x - stick.diameter / 2, y: stick.y - stick.diameter / 2 })
        .opacity(stick.opacity).hitTestBehavior(HitTestMode.None).id(`fivestar-stick-${stick.name}`)
    }
  }

  @Builder
  private settingSlider(label: string, field: string, value: number, min: number, max: number, step: number) {
    Row({ space: 12 }) {
      Text(`${label}  ${value.toFixed(1)}`).width(156).fontSize(13).fontColor('#EAF3F8')
      Slider({ value: value, min: min, max: max, step: step }).layoutWeight(1)
        .onChange((next: number): void => { this.setNumber(field, next); })
    }.width('100%').height(42)
  }
  @Builder
  private settingToggle(label: string, field: string, enabled: boolean) {
    Row() {
      Text(label).fontSize(14).fontColor('#EAF3F8').layoutWeight(1)
      Toggle({ type: ToggleType.Switch, isOn: enabled }).onChange((value: boolean): void => { this.setFlag(field, value); })
    }.width('100%').height(46)
  }
  @Builder
  private settingsPanel() {
    Column({ space: 12 }) {
      Row() {
        Text('触控设置').fontSize(21).fontWeight(FontWeight.Bold).fontColor('#F4F8FC')
        Blank()
        Button('完成').height(42).fontColor('#FFFFFF').backgroundColor('#315A70').onClick((): void => { this.closePanels(); })
      }.width('100%')
      Scroll() {
        Column({ space: 6 }) {
          Button('编辑按键布局').width('100%').height(46).fontColor('#EAF7FF').backgroundColor('#355E73').onClick((): void => { this.beginEditing(); })
          this.settingSlider('按键透明度', 'controlOpacity', this.preferences.controlOpacity, 0.2, 1, 0.05)
          this.settingSlider('按键整体大小', 'controlScale', this.preferences.controlScale, 0.7, 1.5, 0.05)
          Text('视角').fontSize(15).fontWeight(FontWeight.Bold).fontColor('#7EC5EA').width('100%').margin({ top: 10 })
          Row({ space: 8 }) {
            Button('虚拟右摇杆').height(42).layoutWeight(1).fontColor('#FFFFFF').backgroundColor(this.preferences.cameraTouchMode === 0 ? '#3B718B' : '#253E4D').onClick((): void => { this.setNumber('cameraTouchMode', 0); })
            Button('滑动视角').height(42).layoutWeight(1).fontColor('#FFFFFF').backgroundColor(this.preferences.cameraTouchMode !== 0 ? '#3B718B' : '#253E4D').onClick((): void => { this.setNumber('cameraTouchMode', 2); })
          }.width('100%')
          if (this.preferences.cameraTouchMode === 0) {
            this.settingSlider('视角速度', 'lookSensitivity', this.preferences.lookSensitivity, 0.3, 3, 0.1)
            this.settingSlider('瞄准速度', 'aimSensitivity', this.preferences.aimSensitivity, 0.3, 4, 0.1)
          } else {
            this.settingSlider('滑动视角速度', 'mouseLookSensitivity', this.preferences.mouseLookSensitivity, 0.2, 15, 0.1)
            this.settingSlider('滑动瞄准速度', 'mouseAimSensitivity', this.preferences.mouseAimSensitivity, 0.2, 15, 0.1)
          }
          this.settingToggle('开火同时瞄准', 'fireAlsoAims', this.preferences.fireAlsoAims)
          Text('瞄准方式').fontSize(13).fontColor('#AFC5D3').width('100%')
          Flex({ wrap: FlexWrap.Wrap, space: { main: LengthMetrics.vp(6), cross: LengthMetrics.vp(6) } }) {
            ForEach([0, 1, 2, 3], (mode: number): void => {
              Button(['传统锁定', '辅助瞄准', '辅助自由瞄准', '自由瞄准'][mode]).height(40).fontSize(12).fontColor('#FFFFFF')
                .backgroundColor(this.preferences.targetingMode === mode ? '#3B718B' : '#253E4D').onClick((): void => { this.setNumber('targetingMode', mode); })
            })
          }.width('100%')
          Text('操作').fontSize(15).fontWeight(FontWeight.Bold).fontColor('#7EC5EA').width('100%').margin({ top: 10 })
          this.settingToggle('显示奔跑按钮', 'sprintButton', this.preferences.sprintButton)
          this.settingToggle('车辆使用左右转向按键', 'steeringButtons', this.preferences.steeringButtons)
          this.settingToggle('飞机使用右侧油门 / 方向摇杆', 'airplaneRightStick', this.preferences.airplaneRightStick)
          this.settingToggle('连接手柄时隐藏触控', 'hideTouchWithController', this.preferences.hideTouchWithController)
          Text('陀螺仪与倾斜转向').fontSize(15).fontWeight(FontWeight.Bold).fontColor('#7EC5EA').width('100%').margin({ top: 10 })
          Row({ space: 6 }) {
            ForEach([0, 1, 2], (mode: number): void => {
              Button(['陀螺仪关', '瞄准时开启', '步行始终开启'][mode]).height(40).layoutWeight(1).fontSize(12).fontColor('#FFFFFF')
                .backgroundColor(this.preferences.gyroAim === mode ? '#3B718B' : '#253E4D').onClick((): void => { this.setNumber('gyroAim', mode); })
            })
          }.width('100%')
          this.settingSlider('陀螺仪灵敏度', 'gyroSensitivity', this.preferences.gyroSensitivity, 0.1, 4, 0.1)
          this.settingToggle('陀螺仪反转 Y 轴', 'gyroInvertY', this.preferences.gyroInvertY)
          Row({ space: 6 }) {
            ForEach([0, 1, 2], (mode: number): void => {
              Button(['倾斜转向关', '辅助转向', '倾斜主转向'][mode]).height(40).layoutWeight(1).fontSize(12).fontColor('#FFFFFF')
                .backgroundColor(this.preferences.tiltSteer === mode ? '#3B718B' : '#253E4D').onClick((): void => { this.setNumber('tiltSteer', mode); })
            })
          }.width('100%')
          this.settingSlider('满转向角度', 'tiltFullLock', this.preferences.tiltFullLock, 10, 45, 1)
          Button('全键盘').height(44).width('100%').fontColor('#DDF0FC').backgroundColor('#263F50')
            .onClick((): void => { this.closePanels(); this.onOpenKeyboard(); })
          Button('模组按键').height(44).width('100%').fontColor('#DDF0FC').backgroundColor('#263F50')
            .onClick((): void => { this.releaseTouches(); this.panel = 'mods'; })
          if (this.feedback.length > 0) { Text(this.feedback).fontSize(12).fontColor('#B4D2E4').width('100%'); }
        }.width('100%').alignItems(HorizontalAlign.Start)
      }.width('100%').layoutWeight(1).scrollBar(BarState.Auto)
    }.padding(18).width(Math.min(640, this.widthVp - 32)).height(Math.min(560, this.heightVp - 28))
      .backgroundColor('#F0142531').borderRadius(22).border({ width: 1, color: '#56748A' })
      .onTouch((event: TouchEvent): void => { event.stopPropagation(); })
      .onMouse((event: MouseEvent): void => { event.stopPropagation(); })
  }

  @Builder
  private editorToolbar() {
    Column({ space: 6 }) {
      Text(this.selected.length > 0 ? `${this.selectedButton()?.label || this.selected}${this.selectedHidden ? ' · 游戏中隐藏' : ''}` : '拖动按键调整位置 · 双指或滑块调整大小')
        .fontSize(12).fontColor('#FFFFFF').maxLines(1)
      Flex({ wrap: FlexWrap.Wrap, space: { main: LengthMetrics.vp(7), cross: LengthMetrics.vp(6) } }) {
        ForEach(['onFoot', 'car', 'plane'], (context: string): void => {
          Button(this.contextLabel(context)).height(38).fontSize(12).fontColor('#FFFFFF')
            .backgroundColor(this.editorContext === context ? '#3B718B' : '#243D4B').onClick((): void => { this.chooseEditorContext(context); })
        })
        Slider({ value: this.selectedScale, min: 0.6, max: 1.8, step: 0.05 }).width(104).enabled(this.selected.length > 0)
          .onChange((value: number): void => { this.storeSelected(undefined, value); })
        Button(this.selectedHidden ? '显示' : '隐藏').height(38).fontSize(12).fontColor('#FFFFFF').backgroundColor('#294353')
          .enabled(this.selected.length > 0 && ESSENTIAL_CONTROLS.indexOf(this.selected) < 0)
          .onClick((): void => { this.storeSelected(undefined, undefined, !this.selectedHidden); })
        Button('重置').height(38).fontSize(12).fontColor('#FFFFFF').backgroundColor('#294353').onClick((): void => {
          this.engine.resetLayoutGroup(); this.preferences = this.engine.getSettings(); this.selected = ''; this.saveNow(); this.refreshVisuals();
        })
        Button('完成').height(38).fontSize(12).fontColor('#FFFFFF').backgroundColor('#376E87').onClick((): void => { this.finishEditing(); })
      }
    }.padding(10).width(Math.min(680, this.rectWidth() - this.viewportInsets().left - this.viewportInsets().right - 16))
      .backgroundColor('#D9000000').borderRadius(14)
      .position({ x: this.rectX() + Math.max(8, this.viewportInsets().left),
        y: this.rectY() + this.rectHeight() - Math.max(8, this.viewportInsets().bottom) - (this.rectWidth() < 680 ? 144 : 96) })
      .onTouch((event: TouchEvent): void => { event.stopPropagation(); })
      .onMouse((event: MouseEvent): void => { event.stopPropagation(); })
  }

  @Builder
  private modsPanel() {
    Column({ space: 12 }) {
      Row() {
        Text('模组按键').fontSize(20).fontWeight(FontWeight.Bold).fontColor('#FFFFFF')
        Blank()
        Button('全键盘').height(40).fontColor('#FFFFFF').backgroundColor('#315568').onClick((): void => { this.closePanels(); this.onOpenKeyboard(); })
        Button('关闭').height(40).fontColor('#FFFFFF').backgroundColor('#315568').onClick((): void => { this.closePanels(); })
      }.width('100%')
      Scroll() {
        Flex({ wrap: FlexWrap.Wrap, space: { main: LengthMetrics.vp(8), cross: LengthMetrics.vp(8) } }) {
          ForEach(MOD_KEYS, (key: string): void => {
            Text(this.keyLabel(key)).fontSize(14).fontColor('#FFFFFF').textAlign(TextAlign.Center).width(82).height(52)
              .backgroundColor('#294555').borderRadius(12).onTouch((event: TouchEvent): void => { this.modTouch(key, event); })
          }, (key: string): string => key)
        }.width('100%')
      }.width('100%').layoutWeight(1)
    }.padding(18).width(Math.min(600, this.widthVp - 32)).height(Math.min(350, this.heightVp - 28))
      .backgroundColor('#F0142531').borderRadius(22).border({ width: 1, color: '#56748A' })
      .onTouch((event: TouchEvent): void => { event.stopPropagation(); })
      .onMouse((event: MouseEvent): void => { event.stopPropagation(); })
  }

  build() {
    Stack({ alignContent: Alignment.TopStart }) {
      if (this.visible && (!this.hiddenForController() || this.editing)) {
        Stack({ alignContent: Alignment.TopStart }) {
          if (this.editing) { Rect().width('100%').height('100%').fill('#80202B32').hitTestBehavior(HitTestMode.None); }
          ForEach(this.stickDisplays, (stick: FiveStarStickDisplay): void => { this.floatingStick(stick); }, (stick: FiveStarStickDisplay): string => JSON.stringify(stick))
          ForEach(this.buttons, (button: FiveStarButton): void => { this.controlButton(button); }, (button: FiveStarButton): string => `${JSON.stringify(button)}:${this.selected === button.name}`)
        }.width(this.rectWidth()).height(this.rectHeight()).position({ x: this.rectX(), y: this.rectY() })
          .hitTestBehavior(HitTestMode.Block).onTouch((event: TouchEvent): void => { this.surfaceTouch(event); })
          .onMouse((event: MouseEvent): void => { event.stopPropagation(); })
      }
      if (this.editing) { this.editorToolbar(); }
      if (this.panel.length > 0) {
        Stack() {
          Rect().width('100%').height('100%').fill('#40000000')
          if (this.panel === 'settings') { this.settingsPanel(); }
          if (this.panel === 'mods') { this.modsPanel(); }
        }.width('100%').height('100%').onTouch((event: TouchEvent): void => { event.stopPropagation(); })
      }
    }.width('100%').height('100%').hitTestBehavior(HitTestMode.Transparent)
      .onAreaChange((_old: Area, area: Area): void => {
        const width: number = Number(area.width); const height: number = Number(area.height);
        if (width > 0 && height > 0 && (width !== this.widthVp || height !== this.heightVp)) {
          this.releaseTouches(); this.widthVp = width; this.heightVp = height; this.updateViewport(); this.refreshVisuals();
        }
      })
  }
}
