import { bundleManager, common } from '@kit.AbilityKit';
import { BusinessError } from '@kit.BasicServicesKit';
import { fileIo } from '@kit.CoreFileKit';
import { gamePerformance } from '@kit.GameServiceKit';
import { hilog } from '@kit.PerformanceAnalysisKit';

interface SceneRequest {
  sceneID: number;
  reason: string;
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
    this.lifecycle = 'background';
    this.queueScene(0, 'UIAbility.onBackground: end current scene');
  }

  onDestroy(): void {
    if (this.destroyed) {
      return;
    }
    this.destroyed = true;
    this.foreground = false;
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

  private queueScene(sceneID: number, reason: string): void {
    // Keep the latest lifecycle intention while init/update is in flight. This
    // prevents a late init from announcing gameplay after background/destroy.
    this.pending = { sceneID: sceneID, reason: reason };
    this.record('lifecycle', 'queued', reason, sceneID);
    this.drain();
  }

  private drain(): void {
    if (!this.initialized || this.updating || this.pending === undefined) {
      return;
    }
    const request: SceneRequest = this.pending;
    this.pending = undefined;
    if (request.sceneID === this.lastReportedScene) {
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
    this.updating = true;
    this.record('update', 'request', request.reason, request.sceneID);
    this.update(request, scene);
  }

  private async update(request: SceneRequest, scene: gamePerformance.GameSceneInfo): Promise<void> {
    try {
      await gamePerformance.updateGameInfo(scene);
      this.lastReportedScene = request.sceneID;
      this.record('update', 'success', request.reason, request.sceneID, 0, 'success');
    } catch (error) {
      const failure = error as BusinessError;
      this.record('update', 'failure', request.reason, request.sceneID, failure.code, failure.message);
    } finally {
      this.updating = false;
      this.drain();
    }
  }

  private record(event: string, phase: string, reason: string, sceneID: number,
    code?: number, message?: string): void {
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
      message: message
    };
    const json: string = JSON.stringify(evidence);
    if (phase === 'failure') {
      hilog.error(0x4100, 'GamePerformance', '%{public}s', json);
    } else {
      hilog.info(0x4100, 'GamePerformance', '%{public}s', json);
    }
    try {
      const journal = fileIo.openSync(`${this.filesDir}/game-performance-events.jsonl`,
        fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.CREATE | fileIo.OpenMode.APPEND);
      try {
        fileIo.writeSync(journal.fd, `${json}\n`);
      } finally {
        fileIo.closeSync(journal);
      }
      const status = fileIo.openSync(`${this.filesDir}/game-performance-status.json`,
        fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.CREATE | fileIo.OpenMode.TRUNC);
      try {
        fileIo.writeSync(status.fd, json);
      } finally {
        fileIo.closeSync(status);
      }
    } catch (error) {
      hilog.error(0x4100, 'GamePerformance', 'Evidence write failed: %{public}s', String(error));
    }
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

export function gamePerformanceBackground(): void {
  reporter?.onBackground();
}

export function gamePerformanceDestroy(): void {
  reporter?.onDestroy();
}
