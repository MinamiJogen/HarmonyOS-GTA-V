import { TouchAction, TouchProfile, touchProfile } from './TouchLayout';

export interface TouchObserved<T> {
  known: boolean;
  value?: T;
  source?: string;
  reason?: string;
  generation?: number;
  sampledAtMonoMs?: number;
}
export interface TouchSnapshot {
  generation: number;
  seq: number;
  sampledAtMonoMs: number;
  nowMonoMs: number;
  ageMs?: number;
  playerValid?: TouchObserved<boolean>;
  playerControlOn?: TouchObserved<boolean>;
  dead?: TouchObserved<boolean>;
  ragdoll?: TouchObserved<boolean>;
  enteringVehicle?: TouchObserved<boolean>;
  inVehicle?: TouchObserved<boolean>;
  driverSeat?: TouchObserved<boolean>;
  seatValid?: TouchObserved<boolean>;
  turretInputOwned?: TouchObserved<boolean>;
  inMeleeCombat?: TouchObserved<boolean>;
  aiming?: TouchObserved<boolean>;
  inCover?: TouchObserved<boolean>;
  reloading?: TouchObserved<boolean>;
  cutscene?: TouchObserved<boolean>;
  cutsceneBlocksInput?: TouchObserved<boolean>;
  uiOwnsInput?: TouchObserved<boolean>;
  failureMenuOwnsInput?: TouchObserved<boolean>;
  gameShowsConfirm?: TouchObserved<boolean>;
  gameShowsCancel?: TouchObserved<boolean>;
  gameAllowsSkip?: TouchObserved<boolean>;
  loading?: TouchObserved<boolean>;
  worldValid?: TouchObserved<boolean>;
  transport?: TouchObserved<string>;
  ui?: TouchObserved<string>;
  weaponClass?: TouchObserved<string>;
  parachutePhase?: TouchObserved<string>;
  vehicleSubtype?: TouchObserved<string>;
  startupStage?: TouchObserved<string>;
  specialCapabilities?: Record<string, TouchObserved<boolean>>;
}
export interface TouchResolution {
  profile: TouchProfile;
  actions: TouchAction[];
  axisX: string;
  axisY: string;
  directions: string[];
  fresh: boolean;
  manual: boolean;
  generation: number;
  transport: string;
  status: string;
}

/** Conservative finite implementation of the spec's three-valued conditions.
 * Unknown observations never become false. No condition strings are evaluated.
 */
export class TouchStateResolver {
  private snapshot: TouchSnapshot | undefined = undefined;
  private arrivedAt: number = 0;
  private acceptedSequence: number = -1;
  private acceptedGeneration: number = -1;
  private candidate: string = 'UNKNOWN';
  private candidateCount: number = 0;
  private committed: string = 'UNKNOWN';
  private consideredSequence: number = -1;
  private consideredGeneration: number = -1;
  manualProfile: string = '';
  pinnedDetonate: boolean = false;

  accept(json: string, wallMs: number): void {
    if (json.length === 0) { this.snapshot = undefined; return; }
    try {
      const next: TouchSnapshot = JSON.parse(json) as TouchSnapshot;
      if (!Number.isFinite(next.generation) || !Number.isFinite(next.seq) ||
        !Number.isFinite(next.sampledAtMonoMs) || !Number.isFinite(next.nowMonoMs)) {
        this.snapshot = undefined; return;
      }
      if (next.generation === this.acceptedGeneration && next.seq <= this.acceptedSequence) { return; }
      if (this.acceptedGeneration >= 0 && next.generation < this.acceptedGeneration) { return; }
      if (next.generation !== this.acceptedGeneration) {
        this.committed = 'UNKNOWN'; this.candidateCount = 0;
      }
      this.snapshot = next;
      this.arrivedAt = wallMs;
      this.acceptedGeneration = next.generation;
      this.acceptedSequence = next.seq;
    } catch (_) { this.snapshot = undefined; }
  }

  private elapsed(wallMs: number): number { return Math.max(0, wallMs - this.arrivedAt); }
  private known<T>(field: TouchObserved<T> | undefined, wallMs: number): T | undefined {
    const snap: TouchSnapshot | undefined = this.snapshot;
    if (!snap || !field || field.known !== true || !field.source ||
      field.generation !== snap.generation || field.sampledAtMonoMs === undefined) { return undefined; }
    const age: number = snap.nowMonoMs + this.elapsed(wallMs) - field.sampledAtMonoMs;
    if (!Number.isFinite(age) || age < -1 || age > 500) { return undefined; }
    return field.value;
  }
  private valid(wallMs: number): boolean {
    const s: TouchSnapshot | undefined = this.snapshot;
    if (!s) { return false; }
    const age: number = s.nowMonoMs + this.elapsed(wallMs) - s.sampledAtMonoMs;
    return age >= -1 && age <= 500;
  }
  private automatic(wallMs: number, aimLatch: boolean): string {
    const s: TouchSnapshot | undefined = this.snapshot;
    if (!s || !this.valid(wallMs)) { return 'UNKNOWN'; }
    const ui: string | undefined = this.known(s.ui, wallMs);
    const owner: boolean | undefined = this.known(s.uiOwnsInput, wallMs);
    if (owner === true) {
      if (ui === 'phoneCamera') { return 'GAME_PHONE_CAMERA'; }
      if (ui === 'phone') { return 'GAME_PHONE'; }
      if (ui === 'web') { return 'GAME_WEB_POINTER'; }
      if (ui === 'map') { return 'GAME_MAP'; }
      if (ui === 'pause') { return 'GAME_PAUSE'; }
      if (ui === 'startup') { return 'GAME_STARTUP'; }
      return 'UNKNOWN';
    }
    if (this.known(s.cutscene, wallMs) === true && this.known(s.playerControlOn, wallMs) === false &&
      this.known(s.cutsceneBlocksInput, wallMs) === true && owner === false) { return 'BLOCKING_CUTSCENE'; }
    if (this.known(s.dead, wallMs) === true || this.known(s.failureMenuOwnsInput, wallMs) === true) { return 'DEAD_FAILURE'; }
    const transport: string | undefined = this.known(s.transport, wallMs);
    // The bridge only decodes phase names that it can actually observe.
    const phase: string | undefined = this.known(s.parachutePhase, wallMs);
    if (transport === 'freefall' && phase !== undefined) { return 'FREEFALL'; }
    if (transport === 'parachute' && phase !== undefined) { return 'PARACHUTE'; }
    if (this.known(s.enteringVehicle, wallMs) === true && this.known(s.inVehicle, wallMs) !== true) { return 'ENTERING_VEHICLE'; }
    if (this.known(s.ragdoll, wallMs) === true && transport !== undefined &&
      transport !== 'freefall' && transport !== 'parachute') { return 'RAGDOLL'; }
    // Missing player/world data is never interpreted as loading.
    if (this.known(s.loading, wallMs) === true || this.known(s.startupStage, wallMs) === 'loading') { return 'LOADING'; }
    if (this.known(s.playerValid, wallMs) !== true || owner !== false || ui !== 'gameplay') { return 'UNKNOWN'; }
    if (this.known(s.turretInputOwned, wallMs) === true) { return 'VEHICLE_TURRET'; }
    if (transport === 'passenger' && this.known(s.seatValid, wallMs) === true) { return 'VEHICLE_PASSENGER'; }
    if (this.known(s.driverSeat, wallMs) === true) {
      if (transport === 'submarine' && this.known(s.vehicleSubtype, wallMs) !== undefined) { return 'VEHICLE_SUBMARINE'; }
      if (transport === 'heli') { return 'VEHICLE_HELI'; }
      if (transport === 'plane') { return 'VEHICLE_PLANE'; }
      if (transport === 'boat') { return 'VEHICLE_BOAT'; }
      if (transport === 'bicycle') { return 'VEHICLE_BICYCLE'; }
      if (transport === 'motorbike') { return 'VEHICLE_MOTORBIKE'; }
      if (transport === 'car') { return 'VEHICLE_CAR'; }
    }
    if (transport === 'swimUnderwater') { return 'SWIM_UNDERWATER'; }
    if (transport === 'swimSurface') { return 'SWIM_SURFACE'; }
    if (transport !== 'foot') { return 'UNKNOWN'; }
    const weapon: string | undefined = this.known(s.weaponClass, wallMs);
    const aiming: boolean = aimLatch || this.known(s.aiming, wallMs) === true;
    if (weapon === 'sniper' && aiming) { return 'FOOT_SNIPER'; }
    if (weapon === 'firearm' && aiming) { return 'FOOT_AIM'; }
    if (weapon === 'throwable') { return 'FOOT_THROWABLE'; }
    if (weapon === 'melee' || this.known(s.inMeleeCombat, wallMs) === true) { return 'FOOT_MELEE'; }
    if (weapon === 'firearm' || weapon === 'sniper') { return 'FOOT_FIREARM'; }
    if (weapon === 'unarmed') { return 'FOOT_UNARMED'; }
    return 'UNKNOWN';
  }

  private immediate(id: string): boolean {
    return id === 'UNKNOWN' || id === 'DEAD_FAILURE' || id === 'FREEFALL' || id === 'PARACHUTE' ||
      id === 'LOADING' || id.indexOf('GAME_') === 0 || id === 'BLOCKING_CUTSCENE';
  }

  resolve(wallMs: number, aimLatch: boolean = false): TouchResolution {
    const s: TouchSnapshot | undefined = this.snapshot;
    const fresh: boolean = this.valid(wallMs);
    let wanted: string = this.automatic(wallMs, aimLatch);
    const protectedProfile: boolean = wanted === 'DEAD_FAILURE' || wanted === 'BLOCKING_CUTSCENE' ||
      wanted === 'LOADING' || wanted.indexOf('GAME_') === 0;
    const manual: boolean = this.manualProfile.length > 0 && !protectedProfile;
    if (manual) { wanted = this.manualProfile; }
    if (manual || this.immediate(wanted)) {
      this.committed = wanted; this.candidate = wanted; this.candidateCount = 0;
    } else if (s && (s.seq !== this.consideredSequence || s.generation !== this.consideredGeneration)) {
      this.consideredSequence = s.seq; this.consideredGeneration = s.generation;
      this.candidateCount = wanted === this.candidate ? this.candidateCount + 1 : 1;
      this.candidate = wanted;
      if (this.candidateCount >= 2) { this.committed = wanted; }
      // No stale semantic context is carried while a new family is being classified.
      else if (this.committed !== wanted) {
        const aimProfiles: string[] = ['FOOT_FIREARM', 'FOOT_AIM', 'FOOT_SNIPER'];
        if (aimProfiles.indexOf(this.committed) < 0 || aimProfiles.indexOf(wanted) < 0) { this.committed = 'UNKNOWN'; }
      }
    }
    const profile: TouchProfile = touchProfile(this.committed);
    const actions: TouchAction[] = [];
    for (let i = 0; i < profile.slots.length; i++) {
      const action: TouchAction = Object.assign({}, profile.slots[i]);
      if (action.gate === 'task') { continue; } // A153 is a family, never an executable binding.
      if (action.gate === 'failureConfirm' && (!s || this.known(s.failureMenuOwnsInput, wallMs) !== true || this.known(s.gameShowsConfirm, wallMs) !== true)) { continue; }
      if (action.gate === 'failureCancel' && (!s || this.known(s.failureMenuOwnsInput, wallMs) !== true || this.known(s.gameShowsCancel, wallMs) !== true)) { continue; }
      if (action.gate === 'skip' && (!s || this.known(s.gameAllowsSkip, wallMs) !== true)) { continue; }
      if (s && fresh && this.known(s.inCover, wallMs) === true && profile.id.indexOf('FOOT_') === 0) {
        const coverSlot: string = (profile.id === 'FOOT_SNIPER' || profile.id === 'FOOT_MELEE' || profile.id === 'FOOT_THROWABLE') ? 'S5' : 'S1';
        if (action.slot === coverSlot) { action.action = 'A019'; action.label = '离开掩体'; action.icon = '▥'; action.control = 'INPUT_COVER'; action.gesture = 'tap'; action.unverified = false; }
      }
      if (s && action.action === 'A024' && this.known(s.reloading, wallMs) === true) { action.label = '换弹中'; }
      actions.push(action);
    }
    const detonate: boolean = this.pinnedDetonate || !!(s && s.specialCapabilities && this.known(s.specialCapabilities['detonate'], wallMs) === true);
    if (detonate && profile.id !== 'LOADING' && profile.id !== 'DEAD_FAILURE' && profile.id.indexOf('GAME_') !== 0 && profile.id !== 'BLOCKING_CUTSCENE') {
      let duplicate: boolean = false;
      for (let i = 0; i < actions.length; i++) { if (actions[i].control === 'INPUT_DETONATE') { duplicate = true; } }
      if (!duplicate) { actions.push({ slot: 'T0', action: 'A043', label: '引爆', icon: '✹', gesture: 'tap', control: 'INPUT_DETONATE', rawKey: '', uiAction: '', gate: '', unverified: false }); }
    }
    let axisX: string = profile.axisX;
    let axisY: string = profile.axisY;
    if (profile.id === 'DEAD_FAILURE' && (!s || this.known(s.failureMenuOwnsInput, wallMs) !== true)) { axisX = ''; axisY = ''; }
    const status: string = manual ? `手动布局 · ${profile.label}` : profile.id === 'UNKNOWN' ? '状态未确认 · 原始输入' : profile.label;
    return { profile: profile, actions: actions, axisX: axisX, axisY: axisY, directions: profile.directionControls,
      fresh: fresh, manual: manual, generation: s ? s.generation : -1, transport: s ? this.known(s.transport, wallMs) || '' : '', status: status };
  }
}
