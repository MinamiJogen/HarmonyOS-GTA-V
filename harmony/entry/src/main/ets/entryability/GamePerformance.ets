import { bundleManager, common } from '@kit.AbilityKit';
import { BusinessError } from '@kit.BasicServicesKit';
import { gamePerformance } from '@kit.GameServiceKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { systemDateTime } from '@kit.BasicServicesKit';
import { GamePerformanceEvidenceWriter } from '../telemetry/GamePerformanceEvidenceWriter';
const performanceEvidenceWriter: GamePerformanceEvidenceWriter = new GamePerformanceEvidenceWriter();

// Filled from the current process's native snapshot, never from a status file.
export interface NativeGameSample {
  processId: number;
  monotonicNs?: number;
  logicTid?: number;
  submissionTid?: number;
  engineRenderTid?: number;
  sourcePresents: number;
  currentSourceFps?: number;
  sourceSampleAgeMs?: number;
  sourceIntervalSamples: number;
  sourceTargetFps?: number;
}

interface NativeSceneMetrics {
  processId: number;
  monotonicNs: number;
  receivedMs: number;
  sourceAgeMs: number;
  sourcePresents: number;
  keyThread: string;
  currentFps?: number;
  recommendedFps?: number;
  maxFps?: number;
}

function positiveNumber(value?: number): boolean {
  return value !== undefined && value !== null && Number.isFinite(value) && value > 0;
}

function threadId(value?: number): boolean {
  return positiveNumber(value) && Number.isInteger(value!) && value! <= 2147483647;
}

interface SceneRequest {
  sceneID: number;
  reason: string;
  revision: number;
  force: boolean;
}

interface PerformanceEvidence {
  timestamp: string;
  sessionId: string;
  event: string;
  phase: string;
  reason: string;
  bundleName: string;
  appVersion: string;
  sceneID: number;
  initialized: boolean;
  foreground: boolean;
  gameStarted: boolean;
  destroyed: boolean;
  recommendationOnly: boolean;
  engineType: number;
  gameType: number;
  vulkanSupported: boolean;
  code?: number;
  message?: string;
  keyThread?: string;
  currentFps?: number;
  recommendedFps?: number;
  maxFps?: number;
  nativeSampleMonotonicNs?: number;
  nativeSampleProcessId?: number;
  nativeSourcePresents?: number;
  nativeScope: string;
}

class GamePerformanceLifecycle {
  private filesDir: string;
  private sessionId: string = Date.now().toString();
  private bundleName: string = '';
  private appVersion: string = '';
  private initialized: boolean = false;
  private foreground: boolean = false;
  private gameStarted: boolean = false;
  private destroyed: boolean = false;
  private updating: boolean = false;
  private lifecycle: string = 'create';
  private lastReportedScene: number = -1;
  private pending: SceneRequest | undefined = undefined;
  private revision: number = 0;
  private nativeMetrics: NativeSceneMetrics | undefined = undefined;
  private nativeProcessId: number = -1;
  private lastNativeMonotonicNs: number = -1;
  private lastNativeDispatchMs: number = -1;
  private lastNativeKeyThread: string = '';
  private evidenceWriter: GamePerformanceEvidenceWriter = performanceEvidenceWriter;

  constructor(context: common.UIAbilityContext) {
    this.filesDir = context.filesDir;
    this.queueScene(1, 'UIAbility.onCreate: startup');
    this.initialize();
  }

  isDestroyed(): boolean {
    return this.destroyed;
  }

  onForeground(): void {
    if (this.destroyed || this.lifecycle === 'foreground') {
      return;
    }
    this.foreground = true;
    this.revision++;
    this.nativeMetrics = undefined;
    this.lifecycle = 'foreground';
    // UI foreground alone does not establish that gameplay has started.
    this.queueScene(this.gameStarted ? 7 : 1,
      this.gameStarted ? 'UIAbility.onForeground: resume confirmed game' : 'UIAbility.onForeground: startup');
  }

  onGameStarted(): void {
    if (this.destroyed || this.gameStarted) {
      return;
    }
    this.gameStarted = true;
    this.record('lifecycle', 'observed', 'caller confirmed actual game start', this.foreground ? 7 : 0);
    // A delayed game-start callback must not resume a background ability.
    if (this.foreground) {
      this.queueScene(7, 'caller confirmed actual game start while foreground');
    }
  }

  onBackground(): void {
    if (this.destroyed || this.lifecycle === 'background') {
      return;
    }
    this.foreground = false;
    this.revision++;
    this.nativeMetrics = undefined;
    this.lifecycle = 'background';
    this.queueScene(0, 'UIAbility.onBackground: end current scene');
  }

  onDestroy(): void {
    if (this.destroyed) {
      return;
    }
    this.destroyed = true;
    this.foreground = false;
    this.revision++;
    this.nativeMetrics = undefined;
    this.lifecycle = 'destroy';
    this.queueScene(0, 'UIAbility.onDestroy: end current scene');
  }

  private async initialize(): Promise<void> {
    let info: bundleManager.BundleInfo;
    try {
      info = await bundleManager.getBundleInfoForSelf(bundleManager.BundleFlag.GET_BUNDLE_INFO_WITH_APPLICATION);
    } catch (error) {
      const failure = error as BusinessError;
      this.record('bundle-info', 'failure', 'getBundleInfoForSelf failed', -1, failure.code, failure.message);
      return;
    }
    this.bundleName = info.name;
    this.appVersion = info.versionName;
    const packageInfo: gamePerformance.GamePackageInfo = {
      messageType: 0,
      bundleName: this.bundleName,
      appVersion: this.appVersion,
      // Documented OTHER values describe the native RAGE/muguet renderer.
      // Package reporting is separate from the system assistant's UI policy.
      engineType: 200,
      gameType: 200,
      vulkanSupported: true
    };
    this.record('init', 'request', 'real installed bundle/version; no fabricated app ID', -1);
    try {
      await gamePerformance.init(packageInfo);
    } catch (error) {
      const failure = error as BusinessError;
      this.record('init', 'failure', 'gamePerformance.init failed', -1, failure.code, failure.message);
      return;
    }
    this.initialized = true;
    this.record('init', 'success', 'gamePerformance.init resolved', -1, 0, 'success');
    // Report all optional package capabilities explicitly after initialization.
    // Failure must not prevent the already-authorized scene lifecycle updates.
    this.updating = true;
    this.record('package-update', 'request', 'native game, OTHER engine/game type, Vulkan supported', -1);
    try {
      await gamePerformance.updateGameInfo(packageInfo);
      this.record('package-update', 'success', 'game package capabilities reported', -1, 0, 'success');
    } catch (error) {
      const failure = error as BusinessError;
      this.record('package-update', 'failure', 'game package report failed', -1, failure.code, failure.message);
    } finally {
      this.updating = false;
    }
    this.drain();
  }

  onNativeSample(sample: NativeGameSample, recommendedSourceFps?: number, maximumSourceFps?: number): void {
    if (this.destroyed || !this.foreground || !this.gameStarted ||
      !threadId(sample.processId) || !positiveNumber(sample.monotonicNs) ||
      sample.monotonicNs! <= this.lastNativeMonotonicNs ||
      (this.nativeProcessId > 0 && sample.processId !== this.nativeProcessId)) { return; }
    this.nativeProcessId = sample.processId;
    this.lastNativeMonotonicNs = sample.monotonicNs!;
    const now: number = systemDateTime.getUptime(systemDateTime.TimeType.STARTUP, false);
    let keyThread: string = '';
    // The documented 32-character field fits the native submit and game logic
    // TIDs. Engine RenderThread remains separately visible in native telemetry.
    if (threadId(sample.submissionTid)) { keyThread = `render|${sample.submissionTid}`; }
    if (threadId(sample.logicTid)) {
      const logic: string = `logic|${sample.logicTid}`;
      const combined: string = keyThread ? `${keyThread}|${logic}` : logic;
      if (combined.length <= 32) { keyThread = combined; }
    }
    if (keyThread.length > 32) { keyThread = ''; }
    const sourceAge: number | undefined = sample.sourceSampleAgeMs;
    const fresh: boolean = sourceAge !== undefined && sourceAge !== null &&
      Number.isFinite(sourceAge) && sourceAge >= 0 && sourceAge <= 5000;
    const currentFps: number | undefined = fresh && sample.sourceIntervalSamples > 0 &&
      sample.currentSourceFps !== undefined && sample.currentSourceFps !== null &&
      Number.isFinite(sample.currentSourceFps) && sample.currentSourceFps >= 0 && sample.currentSourceFps <= 1000 ?
      sample.currentSourceFps : undefined;
    const maxFps: number | undefined = positiveNumber(maximumSourceFps) && maximumSourceFps! <= 240 ? maximumSourceFps : undefined;
    const recommendedFps: number | undefined = positiveNumber(recommendedSourceFps) && recommendedSourceFps! <= 240 &&
      (maxFps === undefined || recommendedSourceFps! <= maxFps) ? recommendedSourceFps : undefined;
    this.nativeMetrics = { processId: sample.processId, monotonicNs: sample.monotonicNs!, receivedMs: now,
      sourceAgeMs: fresh ? sourceAge! : 5001, sourcePresents: sample.sourcePresents,
      keyThread: keyThread, currentFps: currentFps, recommendedFps: recommendedFps, maxFps: maxFps };
    if (this.lastNativeDispatchMs >= 0 && now - this.lastNativeDispatchMs < 10000 && keyThread === this.lastNativeKeyThread) { return; }
    this.lastNativeDispatchMs = now;
    this.lastNativeKeyThread = keyThread;
    this.queueScene(7, 'current native source frame rate and live OS thread IDs', true);
  }

  private queueScene(sceneID: number, reason: string, force: boolean = false): void {
    // Keep the latest lifecycle intention while init/update is in flight. This
    // prevents a late init from announcing gameplay after background/destroy.
    this.pending = { sceneID: sceneID, reason: reason, revision: this.revision, force: force };
    this.record('lifecycle', 'queued', reason, sceneID);
    this.drain();
  }

  private drain(): void {
    if (!this.initialized || this.updating || this.pending === undefined) {
      return;
    }
    const request: SceneRequest = this.pending;
    this.pending = undefined;
    if (request.revision !== this.revision || (request.sceneID !== 0 && !this.foreground) ||
      (request.sceneID === this.lastReportedScene && !request.force)) {
      return;
    }
    const scene: gamePerformance.GameSceneInfo = {
      messageType: 2,
      sceneID: request.sceneID,
      importanceLevel: request.sceneID === 0 ? 1 : 3
    };
    if (request.sceneID !== 0) {
      // These are documented recommendations, not forced clocks or proof that
      // the system has classified the application as a game.
      scene.recommendedCpuLevel = 3;
      scene.recommendedGpuLevel = 3;
    }
    const metrics: NativeSceneMetrics | undefined = request.sceneID === 7 ? this.nativeMetrics : undefined;
    if (metrics !== undefined) {
      const age: number = systemDateTime.getUptime(systemDateTime.TimeType.STARTUP, false) - metrics.receivedMs;
      if (age >= 0 && age <= 5000) {
        if (metrics.keyThread) { scene.keyThread = metrics.keyThread; }
        if (metrics.sourceAgeMs + age <= 5000) { scene.currentFps = metrics.currentFps; }
        scene.recommendedFps = metrics.recommendedFps;
        scene.maxFps = metrics.maxFps;
      }
    }
    this.updating = true;
    this.record('update', 'request', request.reason, request.sceneID, undefined, undefined, scene, metrics);
    this.update(request, scene, metrics);
  }

  private async update(request: SceneRequest, scene: gamePerformance.GameSceneInfo, metrics?: NativeSceneMetrics): Promise<void> {
    try {
      await gamePerformance.updateGameInfo(scene);
      const current: boolean = request.revision === this.revision;
      if (current) { this.lastReportedScene = request.sceneID; }
      this.record('update', current ? 'success' : 'retired-success', request.reason, request.sceneID, 0,
        current ? 'success' : 'API completed after lifecycle changed; latest scene is queued', scene, metrics);
    } catch (error) {
      const failure = error as BusinessError;
      this.record('update', 'failure', request.reason, request.sceneID, failure.code, failure.message, scene, metrics);
    } finally {
      this.updating = false;
      this.drain();
    }
  }

  private record(event: string, phase: string, reason: string, sceneID: number,
    code?: number, message?: string, scene?: gamePerformance.GameSceneInfo, metrics?: NativeSceneMetrics): void {
    const evidence: PerformanceEvidence = {
      timestamp: new Date().toISOString(),
      sessionId: this.sessionId,
      event: event,
      phase: phase,
      reason: reason,
      bundleName: this.bundleName,
      appVersion: this.appVersion,
      sceneID: sceneID,
      initialized: this.initialized,
      foreground: this.foreground,
      gameStarted: this.gameStarted,
      destroyed: this.destroyed,
      recommendationOnly: true,
      engineType: 200,
      gameType: 200,
      vulkanSupported: true,
      code: code,
      message: message,
      keyThread: scene?.keyThread, currentFps: scene?.currentFps,
      recommendedFps: scene?.recommendedFps, maxFps: scene?.maxFps,
      nativeSampleMonotonicNs: metrics?.monotonicNs, nativeSampleProcessId: metrics?.processId,
      nativeSourcePresents: metrics?.sourcePresents,
      nativeScope: 'deduplicated source present calls; OS submit/logic threads; API success is not GPU completion, display timing, fixed clocks or performance gain'
    };
    const json: string = JSON.stringify(evidence);
    if (phase === 'failure') {
      hilog.error(0x4100, 'GamePerformance', '%{public}s', json);
    } else {
      hilog.info(0x4100, 'GamePerformance', '%{public}s', json);
    }
    this.evidenceWriter.enqueue(this.filesDir, json);
  }
}

let reporter: GamePerformanceLifecycle | undefined = undefined;

export function gamePerformanceCreate(context: common.UIAbilityContext): void {
  if (reporter === undefined || reporter.isDestroyed()) {
    reporter = new GamePerformanceLifecycle(context);
  }
}

export function gamePerformanceForeground(): void {
  reporter?.onForeground();
}

// Call only after the runtime has actually confirmed game start, never merely
// because an ability window or loading page exists.
export function gamePerformanceGameStarted(): void {
  reporter?.onGameStarted();
}

export function gamePerformanceNativeSample(sample: NativeGameSample,
  recommendedSourceFps?: number, maximumSourceFps?: number): void {
  reporter?.onNativeSample(sample, recommendedSourceFps, maximumSourceFps);
}

export function gamePerformanceBackground(): void {
  reporter?.onBackground();
}

export function gamePerformanceDestroy(): void {
  reporter?.onDestroy();
}
