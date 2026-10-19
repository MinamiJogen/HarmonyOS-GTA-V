import game, { GameStatus, VulkanBufferProbeRequest } from 'libharmony_probe.so';
import { common } from '@kit.AbilityKit';
import { fileIo } from '@kit.CoreFileKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import inputDevice from '@ohos.multimodalInput.inputDevice';
import pointer from '@ohos.multimodalInput.pointer';
import { systemDateTime } from '@kit.BasicServicesKit';
import { gamePerformanceGameStarted } from '../entryability/GamePerformance';
import { CpuSample, GpuSample, readCpuSample, readGpuSample } from '../telemetry/GameTelemetry';
import { applyGameDefaults } from '../config/GameDefaults';
import {
  DownloadGamePackage, GamePackageDirectory, getGamePackageDirectory, listGamePackages,
  openGamePackageDirectory, selectGamePackage
} from '../resources/PublicGamePackages';

interface ResourceCheck {
  ready: boolean;
  error?: string;
  build?: string;
}

interface ResourceImportStatus {
  active?: boolean;
  phase?: string;
  completedBytes?: number;
  totalBytes?: number;
  files?: number;
  error?: string;
  message?: string;
  ready?: boolean;
  cancelled?: boolean;
  warning?: string;
}

interface SpatialStatus {
  activeGameRenderSize?: number[];
  activeOutputSize?: number[];
  mode?: string;
}

interface RuntimeStatus {
  status?: string;
  stage?: string;
  message?: string;
  error?: string;
  presents?: number;
  inputEvents?: number;
  rssMB?: number;
  heapVirtualMB?: number;
  heapResidentMB?: number;
  worldPlayerObserved?: boolean;
  renderSize?: number[];
  spatialUpscale?: SpatialStatus;
}

interface PerformanceSnapshot {
  visible: boolean;
  fps: number;
  appCpuPercent: number;
  systemCpuPercent: number;
  gpuLoadLevel: number;
  gpuFrequencyMHz: number;
  gpuMemoryMB: number;
  uptimeMs: number;
}

interface UiGameStatus extends GameStatus {
  uiPerformance?: PerformanceSnapshot;
}

interface BufferProbeDisplayResult {
  status?: string;
  completedIterations?: number;
  iterationsCompleted?: number;
  requestedIterations?: number;
  poolPolicy?: string;
  error?: string;
}

@Entry
@Component
struct Index {
  @State private gamePageVisible: boolean = false;
  @State private resourceChecking: boolean = true;
  @State private resourceReady: boolean = false;
  @State private runtimeLinked: boolean = false;
  @State private resourceMessage: string = '正在检查已安装的游戏资源…';
  @State private directoryLabel: string = '正在准备公共下载目录…';
  @State private directoryAvailable: boolean = false;
  @State private packageList: DownloadGamePackage[] = [];
  @State private packageListLoading: boolean = false;
  @State private importing: boolean = false;
  @State private importProgress: number = 0;
  @State private importMessage: string = '';
  @State private importFileName: string = '';
  private packageDirectory: GamePackageDirectory | undefined = undefined;
  private inputRegistered: boolean = false;
  private importPollTimer: number = -1;
  private importCancelRequested: boolean = false;
  @State private statusText: string = '等待原生游戏窗口';
  @State private detailText: string = '';
  @State private controlsVisible: boolean = true;
  @State private diagnosticsVisible: boolean = false;
  @State private gameRunning: boolean = false;
  @State private fpsText: string = '采样中';
  @State private frameTimeText: string = '—';
  @State private appCpuText: string = '采样中';
  @State private systemCpuText: string = '采样中';
  @State private gpuLoadText: string = '采样中';
  @State private gpuFrequencyText: string = '—';
  @State private gpuMemoryText: string = '采样中';
  @State private memoryText: string = '—';
  @State private resolutionText: string = '—';
  @State private outputResolutionText: string = '—';
  @State private mouseLook: boolean = true;
  @StorageLink('gameAppForeground') @Watch('onAppForegroundChanged') private appForeground: boolean = false;
  @StorageLink('gameImportRequestedUri') @Watch('onImportUriChanged') private requestedImportUri: string = '';
  @State private bufferProbeMode: boolean = false;
  @State private bufferProbeOutput: string = '';
  @State private probeTitle: string = 'Vulkan 缓冲区独立诊断';
  @State private probeResultFileName: string = 'vulkan-buffer-probe-result.json';
  private pollTimer: number = -1;
  private filesDirectory: string = '';
  private lastEvidence: string = '';
  private previousPresents: number = -1;
  private previousSampleMs: number = 0;
  private fps: number = -1;
  private appCpuPercent: number = -1;
  private systemCpuPercent: number = -1;
  private gpuLoadLevel: number = -1;
  private gpuFrequencyMHz: number = -1;
  private gpuMemoryMB: number = -1;
  private telemetryEpoch: number = 0;
  private lastTelemetryMs: number = -1;
  private cpuPending: boolean = false;
  private gpuPending: boolean = false;
  private pageActive: boolean = false;
  private lastMouseX: number = 0;
  private lastMouseY: number = 0;
  private mousePositionKnown: boolean = false;
  private loggedKeyboard: boolean = false;
  private loggedMouse: boolean = false;
  private loggedAxis: boolean = false;
  private gameFocused: boolean = false;
  private lastKeyCode: number = -1;
  private lastAxisVertical: number = 0;
  private lastAxisStep: number = 0;
  private lastAxisNotches: number = 0;
  private externalButtons: boolean[] = [false, false, false, false, false];
  private inputListener: (change: inputDevice.DeviceListener) => void = (change: inputDevice.DeviceListener) => {
    if (change.type === 'remove') {
      this.resetInput();
      if (this.appForeground && this.gameFocused) { game.setInputFocus(true); }
      hilog.info(0x4100, 'GameInput', 'INPUT_DEVICE_REMOVED releaseAll=true');
    }
  };

  aboutToAppear(): void {
    this.pageActive = true;
    const context = this.getUIContext().getHostContext() as common.UIAbilityContext;
    this.filesDirectory = context.filesDir;
    hilog.info(0x4100, 'GameRuntime', 'RESOURCE_PATH filesDir=%{public}s cacheDir=%{public}s resourceRoot=%{public}s',
      context.filesDir, context.cacheDir, `${context.filesDir}/game`);
    const markerPath: string = `${context.filesDir}/vulkan-buffer-probe-request.json`;
    if (fileIo.accessSync(markerPath)) {
      this.bufferProbeMode = true;
      this.statusText = 'Vulkan 缓冲区诊断正在后台执行';
      hilog.info(0x4100, 'VulkanBufferProbe', 'DIAGNOSTIC_MARKER_FOUND path=%{public}s gameSkipped=true', markerPath);
      try {
        const requestText: string = fileIo.readTextSync(markerPath);
        const request: VulkanBufferProbeRequest = JSON.parse(requestText) as VulkanBufferProbeRequest;
        if (request.probe_kind === 'wgpu_atlas') {
          this.probeTitle = 'wgpu 字体图集独立诊断';
          this.probeResultFileName = 'wgpu-atlas-probe-result.json';
          this.statusText = 'wgpu 字体图集诊断正在后台执行';
        } else if (request.probe_kind === 'depth_downsample') {
          this.probeTitle = '深度下采样 GPU 差分诊断';
          this.probeResultFileName = 'depth-downsample-probe-result.json';
          this.statusText = '原版与优化版 GPU 输出正在逐字节核对';
        }
        this.bufferProbeOutput = `请求：${requestText}`;
        game.runVulkanBufferProbe(context.filesDir, request).then((json: string) => {
          const report: BufferProbeDisplayResult = JSON.parse(json) as BufferProbeDisplayResult;
          this.bufferProbeOutput = json;
          this.statusText = report.status === 'pass' ?
            `诊断完成：通过 ${report.iterationsCompleted || report.completedIterations || 0} 轮` : '诊断完成：未通过';
          hilog.info(0x4100, 'VulkanBufferProbe', 'DIAGNOSTIC_UI_COMPLETE status=%{public}s output=%{public}s',
            report.status || 'unknown', `${context.filesDir}/${this.probeResultFileName}`);
        }).catch((error: Error) => { this.saveBufferProbeFailure(String(error)); });
      } catch (error) {
        this.saveBufferProbeFailure(String(error));
      }
      return;
    }
    // configureGame starts the native guest. Keep it and the XComponent behind
    // resource validation so a failed first launch cannot lock the process.
    this.runtimeLinked = game.gameStatus().runtimeLinked;
    this.checkResources();
    this.refreshPackages();
  }

  aboutToDisappear(): void {
    this.pageActive = false;
    this.telemetryEpoch += 1;
    if (this.pollTimer >= 0) {
      clearInterval(this.pollTimer);
      this.pollTimer = -1;
    }
    if (!this.bufferProbeMode) {
      if (this.importing) { this.importCancelRequested = true; game.cancelGameImport(); }
      if (this.importPollTimer >= 0) { clearInterval(this.importPollTimer); this.importPollTimer = -1; }
      if (this.inputRegistered) {
        inputDevice.off('change', this.inputListener);
        this.inputRegistered = false;
      }
      if (this.gamePageVisible) { this.resetInput(); game.stopGame(); }
    }
  }

  private async checkResources(): Promise<void> {
    this.resourceChecking = true;
    try {
      const result: ResourceCheck = JSON.parse(await game.checkGameResources(this.filesDirectory)) as ResourceCheck;
      if (!this.pageActive) { return; }
      this.resourceReady = result.ready;
      this.resourceMessage = result.ready ? '游戏资源已就绪，存档保存在应用内。' :
        '尚未找到完整资源。将游戏 ZIP 放入下载目录，或从文件管理中选择。';
      if (!result.ready && result.error) {
        hilog.info(0x4100, 'GameImport', 'RESOURCE_CHECK %{public}s', result.error);
      }
    } catch (error) {
      if (this.pageActive) { this.resourceReady = false; this.resourceMessage = String(error); }
    } finally {
      if (this.pageActive) { this.resourceChecking = false; this.onImportUriChanged(); }
    }
  }

  private onImportUriChanged(): void {
    if (!this.pageActive || this.resourceChecking || !this.requestedImportUri || this.bufferProbeMode) { return; }
    const uri: string = this.requestedImportUri;
    this.requestedImportUri = '';
    if (this.gamePageVisible || this.importing) {
      this.getUIContext().getPromptAction().showToast({
        message: this.gamePageVisible ? '请重新打开应用，回到首页后再导入游戏包。' : '正在导入，请完成后再选择其他文件。'
      });
      return;
    }
    if (!uri.startsWith('file://')) {
      this.importMessage = '未取得文件访问权限，请点击“选择游戏 ZIP”重新选择。';
      return;
    }
    this.importPackage(uri, '文件管理打开的游戏 ZIP');
  }

  private async refreshPackages(): Promise<void> {
    if (this.packageListLoading || this.importing || this.gamePageVisible) { return; }
    this.packageListLoading = true;
    try {
      const context = this.getUIContext().getHostContext() as common.UIAbilityContext;
      const directory: GamePackageDirectory = await getGamePackageDirectory(context);
      const packages: DownloadGamePackage[] = await listGamePackages(directory);
      if (!this.pageActive) { return; }
      this.packageDirectory = directory;
      this.directoryLabel = directory.displayName;
      this.directoryAvailable = true;
      this.packageList = packages;
    } catch (error) {
      if (this.pageActive) {
        this.directoryAvailable = false;
        this.directoryLabel = '公共目录暂不可用，请点击“选择游戏 ZIP”。';
        hilog.warn(0x4100, 'GameImport', 'DOWNLOAD_DIRECTORY_FAILED %{public}s', String(error));
      }
    } finally {
      if (this.pageActive) { this.packageListLoading = false; }
    }
  }

  private async openDownloadDirectory(): Promise<void> {
    try {
      const context = this.getUIContext().getHostContext() as common.UIAbilityContext;
      const directory: GamePackageDirectory = this.packageDirectory || await getGamePackageDirectory(context);
      await openGamePackageDirectory(context, directory);
    } catch (error) {
      this.importMessage = `无法打开文件管理，请手动打开 ${this.directoryLabel}，或点击“选择游戏 ZIP”。`;
      hilog.warn(0x4100, 'GameImport', 'OPEN_DOWNLOAD_FAILED %{public}s', String(error));
    }
  }

  private async choosePackage(): Promise<void> {
    if (this.importing || this.resourceChecking || this.gamePageVisible) { return; }
    try {
      const context = this.getUIContext().getHostContext() as common.UIAbilityContext;
      const uri: string = await selectGamePackage(context, this.packageDirectory?.uri || '');
      if (uri && this.pageActive) { await this.importPackage(uri, '所选游戏 ZIP'); }
    } catch (error) {
      if (this.pageActive) { this.importMessage = `未选择文件或文件无法打开：${String(error)}`; }
    }
  }

  private updateImportProgress(): void {
    try {
      const status: ResourceImportStatus = JSON.parse(game.getGameImportStatus()) as ResourceImportStatus;
      const total: number = status.totalBytes || 0;
      this.importProgress = total > 0 ? Math.min(99, Math.max(0, (status.completedBytes || 0) * 100 / total)) : 0;
      const phase: string = status.phase || '';
      this.importMessage = `${this.importPhaseName(phase)} · ${this.importProgress.toFixed(0)}%`;
    } catch (_) { /* Keep the last useful progress snapshot. */ }
  }

  private importPhaseName(phase: string): string {
    switch (phase) {
      case 'queued': return '等待导入';
      case 'checking': return '检查压缩包';
      case 'extracting': return '解压游戏资源';
      case 'validating': return '校验资源完整性';
      case 'committing': return '启用新资源';
      case 'cancelling': return '正在取消';
      case 'cancelled': return '已取消';
      case 'completed': return '导入完成';
      default: return '正在处理游戏包';
    }
  }

  private async importPackage(source: string, name: string): Promise<void> {
    if (this.importing || this.resourceChecking || this.gamePageVisible) { return; }
    this.importing = true;
    this.importCancelRequested = false;
    this.importFileName = name;
    this.importMessage = '正在打开游戏包…';
    this.importProgress = 0;
    let file: fileIo.File | undefined = undefined;
    try {
      file = await fileIo.open(source, fileIo.OpenMode.READ_ONLY);
      if (!this.pageActive) { return; }
      if (this.importCancelRequested) { throw new Error('已取消导入'); }
      this.importPollTimer = setInterval(() => { this.updateImportProgress(); }, 500);
      const result: ResourceImportStatus = JSON.parse(await game.importGameResources(file.fd, this.filesDirectory)) as ResourceImportStatus;
      if (!this.pageActive) { return; }
      if (result.cancelled) { throw new Error('已取消导入'); }
      if (result.ready !== true || result.error) { throw new Error(result.error || '游戏资源完整性校验未通过'); }
      if (this.importPollTimer >= 0) { clearInterval(this.importPollTimer); this.importPollTimer = -1; }
      this.importProgress = 100;
      this.importMessage = result.warning ? `导入完成。${result.warning}` : '导入完成。原 ZIP 已保留。';
      await this.checkResources();
    } catch (error) {
      if (this.pageActive) { this.importMessage = `导入未完成：${String(error)}。已有资源和存档保持不变。`; }
    } finally {
      if (file) {
        try { await fileIo.close(file); } catch (error) {
          hilog.warn(0x4100, 'GameImport', 'SOURCE_CLOSE_FAILED %{public}s', String(error));
        }
      }
      if (this.importPollTimer >= 0) { clearInterval(this.importPollTimer); this.importPollTimer = -1; }
      if (this.pageActive) { this.importing = false; }
    }
  }

  private beginGame(): void {
    if (this.importing || this.resourceChecking || !this.resourceReady || !this.runtimeLinked || this.gamePageVisible) { return; }
    try {
      applyGameDefaults(this.filesDirectory);
      game.configureGame(this.filesDirectory);
      inputDevice.on('change', this.inputListener);
      this.inputRegistered = true;
      this.gamePageVisible = true;
      this.refreshStatus();
      this.pollTimer = setInterval(() => { this.refreshStatus(); }, 1000);
    } catch (error) {
      if (this.inputRegistered) { inputDevice.off('change', this.inputListener); this.inputRegistered = false; }
      game.stopGame();
      this.resourceMessage = `游戏初始化失败：${String(error)}`;
      hilog.error(0x4100, 'GameImport', 'START_FAILED %{public}s', String(error));
    }
  }

  private packageSize(bytes: number): string {
    return bytes >= 1000000000 ? `${(bytes / 1000000000).toFixed(2)} GB` : `${(bytes / 1000000).toFixed(1)} MB`;
  }

  private focusGame(): void {
    if (!this.appForeground) { return; }
    this.getUIContext().getFocusController().requestFocus('game-input');
  }

  private onAppForegroundChanged(): void {
    if (!this.gamePageVisible) {
      if (this.appForeground && this.pageActive && !this.bufferProbeMode) { this.refreshPackages(); }
      return;
    }
    this.telemetryEpoch += 1;
    this.previousPresents = -1;
    this.previousSampleMs = 0;
    this.lastTelemetryMs = -1;
    this.fps = -1;
    this.fpsText = '采样中';
    this.frameTimeText = '—';
    hilog.info(0x4100, 'GameInput', 'UI_FOREGROUND %{public}s', String(this.appForeground));
    this.resetInput();
    if (this.appForeground) {
      game.setInputFocus(true);
      this.focusGame();
    } else {
      this.gameFocused = false;
    }
  }

  private toggleStatus(): void {
    this.diagnosticsVisible = !this.diagnosticsVisible;
    this.telemetryEpoch += 1;
    if (this.diagnosticsVisible) {
      this.appCpuPercent = -1;
      this.systemCpuPercent = -1;
      this.gpuLoadLevel = -1;
      this.gpuFrequencyMHz = -1;
      this.gpuMemoryMB = -1;
      this.appCpuText = '采样中';
      this.systemCpuText = '采样中';
      this.gpuLoadText = '采样中';
      this.gpuFrequencyText = '—';
      this.gpuMemoryText = '采样中';
      this.lastTelemetryMs = -1;
    }
    this.refreshStatus();
  }

  private samplePerformance(presents: number): number {
    const now: number = systemDateTime.getUptime(systemDateTime.TimeType.STARTUP, false);
    const elapsed: number = now - this.previousSampleMs;
    if (this.appForeground && this.previousPresents >= 0 && presents >= this.previousPresents &&
      elapsed >= 250 && elapsed <= 5000) {
      this.fps = (presents - this.previousPresents) * 1000 / elapsed;
      this.fpsText = this.fps.toFixed(1);
      this.frameTimeText = this.fps > 0 ? `${(1000 / this.fps).toFixed(1)} ms` : '—';
    }
    if (elapsed >= 250 || this.previousPresents < 0 || presents < this.previousPresents) {
      this.previousPresents = presents;
      this.previousSampleMs = now;
    }
    if (!this.diagnosticsVisible || !this.appForeground || !this.pageActive ||
      (this.lastTelemetryMs >= 0 && now - this.lastTelemetryMs < 2000)) { return now; }
    this.lastTelemetryMs = now;
    const epoch: number = this.telemetryEpoch;
    if (!this.cpuPending) {
      this.cpuPending = true;
      readCpuSample().then((sample: CpuSample) => {
        if (!this.pageActive || !this.appForeground || !this.diagnosticsVisible || epoch !== this.telemetryEpoch) { return; }
        this.appCpuPercent = sample.appRatio >= 0 ? sample.appRatio * 100 : -1;
        this.systemCpuPercent = sample.systemRatio >= 0 ? sample.systemRatio * 100 : -1;
        this.appCpuText = this.appCpuPercent >= 0 ? `${this.appCpuPercent.toFixed(1)}%` : '暂不可用';
        this.systemCpuText = this.systemCpuPercent >= 0 ? `${this.systemCpuPercent.toFixed(1)}%` : '暂不可用';
      }).catch(() => {
        if (epoch === this.telemetryEpoch) { this.appCpuText = '暂不可用'; this.systemCpuText = '暂不可用'; }
      }).finally(() => { this.cpuPending = false; });
    }
    if (!this.gpuPending) {
      this.gpuPending = true;
      readGpuSample().then((sample: GpuSample) => {
        if (!this.pageActive || !this.appForeground || !this.diagnosticsVisible || epoch !== this.telemetryEpoch) { return; }
        this.gpuLoadLevel = sample.loadLevel;
        this.gpuFrequencyMHz = sample.frequencyMHz;
        this.gpuMemoryMB = sample.memoryMB;
        this.gpuLoadText = sample.loadLevel >= 1 ? `${sample.loadLevel} / 10` : '暂不可用';
        this.gpuFrequencyText = sample.frequencyMHz > 0 ? `${sample.frequencyMHz.toFixed(0)} MHz` : '—';
        this.gpuMemoryText = sample.memoryMB >= 0 ? `${sample.memoryMB.toFixed(0)} MiB` : '暂不可用';
      }).catch(() => {
        if (epoch === this.telemetryEpoch) { this.gpuLoadText = '暂不可用'; this.gpuMemoryText = '暂不可用'; }
      }).finally(() => { this.gpuPending = false; });
    }
    return now;
  }

  private resetInput(): void {
    game.setInputFocus(false);
    this.mousePositionKnown = false;
    this.externalButtons = [false, false, false, false, false];
    this.showPointer(true);
  }

  private showPointer(visible: boolean): void {
    try {
      pointer.setPointerVisibleSync(visible);
    } catch (error) {
      hilog.warn(0x4100, 'GameInput', 'POINTER_VISIBILITY failed %{public}s', String(error));
    }
  }

  private toggleMouseLook(): void {
    this.mouseLook = !this.mouseLook;
    this.mousePositionKnown = false;
    this.showPointer(!this.mouseLook);
    this.focusGame();
  }

  private externalKey(event: KeyEvent): boolean {
    this.lastKeyCode = event.keyCode;
    if (!this.appForeground) { return false; }
    if (event.type !== KeyType.Down && event.type !== KeyType.Up) { return false; }
    // F8 toggles camera look and the visible menu cursor without changing Escape's game action.
    if (event.keyCode === 2097) {
      if (event.type === KeyType.Up) { this.toggleMouseLook(); }
      return true;
    }
    const suppressText: boolean = event.metaKey !== 0 ||
      (event.getModifierKeyState ? event.getModifierKeyState(['Ctrl']) : false);
    const handled: boolean = game.sendKeyEvent(event.keyCode, event.type === KeyType.Down,
      event.unicode || 0, suppressText);
    if (handled && !this.loggedKeyboard) {
      this.loggedKeyboard = true;
      hilog.info(0x4100, 'GameInput', 'EXTERNAL_KEY firstCode=%{public}d deviceId=%{public}d', event.keyCode, event.deviceId);
    }
    return handled;
  }

  private externalMouse(event: MouseEvent): void {
    if (!this.appForeground) { return; }
    if (event.action === MouseAction.CANCEL || event.action === MouseAction.LEAVE_WINDOW) {
      this.releaseMouseButtons();
      this.mousePositionKnown = false;
      this.showPointer(true);
      return;
    }
    if (event.action === MouseAction.Press) { this.focusGame(); }
    if (event.action === MouseAction.Press || event.action === MouseAction.Release) {
      // A device can click at a new position without delivering a preceding Move.
      game.sendPointer(this.getUIContext().vp2px(event.x), this.getUIContext().vp2px(event.y), 0, 0, this.mouseLook);
    }
    if (event.action === MouseAction.Move) {
      const rawX: number = event.rawDeltaX || 0;
      const rawY: number = event.rawDeltaY || 0;
      const useRaw: boolean = rawX !== 0 || rawY !== 0;
      const dx: number = useRaw ? rawX : (this.mousePositionKnown ? event.x - this.lastMouseX : 0);
      const dy: number = useRaw ? rawY : (this.mousePositionKnown ? event.y - this.lastMouseY : 0);
      game.sendPointer(this.getUIContext().vp2px(event.x), this.getUIContext().vp2px(event.y), dx, dy, this.mouseLook);
      this.showPointer(!this.mouseLook);
      if (!this.loggedMouse) {
        this.loggedMouse = true;
        hilog.info(0x4100, 'GameInput', 'EXTERNAL_POINTER rawAvailable=%{public}s sourceTool=%{public}d',
          String(event.rawDeltaX !== undefined), event.sourceTool);
      }
    }
    this.lastMouseX = event.x;
    this.lastMouseY = event.y;
    this.mousePositionKnown = true;
    const button: number = this.domButton(event.button);
    if (button >= 0 && (event.action === MouseAction.Press || event.action === MouseAction.Release)) {
      const pressed: boolean = event.action === MouseAction.Press;
      game.sendMouseButton(button, pressed);
      this.externalButtons[button] = pressed;
    }
    // Reconcile the full button state so a missed release cannot leave firing held.
    if (event.pressedButtons) {
      const buttons: MouseButton[] = [MouseButton.Left, MouseButton.Middle, MouseButton.Right, MouseButton.Back, MouseButton.Forward];
      buttons.forEach((value: MouseButton) => {
        const dom: number = this.domButton(value);
        if (event.pressedButtons && !event.pressedButtons.includes(value) && this.externalButtons[dom]) {
          game.sendMouseButton(dom, false);
          this.externalButtons[dom] = false;
        }
      });
    }
    event.stopPropagation();
  }

  private domButton(button: MouseButton): number {
    if (button === MouseButton.Left) { return 0; }
    if (button === MouseButton.Middle) { return 1; }
    if (button === MouseButton.Right) { return 2; }
    if (button === MouseButton.Back) { return 3; }
    if (button === MouseButton.Forward) { return 4; }
    return -1;
  }

  private releaseMouseButtons(): void {
    for (let button: number = 0; button < 5; button++) {
      if (this.externalButtons[button]) { game.sendMouseButton(button, false); }
      this.externalButtons[button] = false;
    }
  }

  private externalAxis(event: AxisEvent): void {
    if (!this.appForeground) { return; }
    if (event.action !== AxisAction.BEGIN && event.action !== AxisAction.UPDATE) { return; }
    const vertical: number = event.getVerticalAxisValue();
    // Mouse uses its reported wheel step; touchpad scrolling keeps fractional movement.
    const step: number = event.scrollStep || 100;
    this.lastAxisVertical = vertical;
    this.lastAxisStep = event.scrollStep || 0;
    this.lastAxisNotches = -vertical / step;
    if (vertical !== 0) { game.sendWheel(-vertical / step); }
    if (!this.loggedAxis && vertical !== 0) {
      this.loggedAxis = true;
      hilog.info(0x4100, 'GameInput', 'EXTERNAL_AXIS sourceTool=%{public}d scrollStep=%{public}f vertical=%{public}f notches=%{public}f',
        event.sourceTool, step, vertical, this.lastAxisNotches);
    }
  }

  private saveBufferProbeFailure(message: string): void {
    this.statusText = 'Vulkan 诊断执行失败';
    this.bufferProbeOutput = JSON.stringify({ status: 'fail', error: message });
    try {
      const file = fileIo.openSync(`${this.filesDirectory}/${this.probeResultFileName}`,
        fileIo.OpenMode.READ_WRITE | fileIo.OpenMode.CREATE | fileIo.OpenMode.TRUNC);
      try {
        fileIo.writeSync(file.fd, this.bufferProbeOutput);
      } finally {
        fileIo.closeSync(file);
      }
    } catch (error) {
      hilog.error(0x4100, 'VulkanBufferProbe', 'DIAGNOSTIC_RESULT_WRITE_FAILED %{public}s', String(error));
    }
    hilog.error(0x4100, 'VulkanBufferProbe', 'DIAGNOSTIC_FAILED %{public}s', message);
  }

  private refreshStatus(): void {
    try {
      const status: UiGameStatus = game.gameStatus() as UiGameStatus;
      status.uiAppForeground = this.appForeground;
      status.uiGameFocused = this.gameFocused;
      status.uiMouseLook = this.mouseLook;
      status.uiLastKeyCode = this.lastKeyCode;
      status.uiLastAxisVertical = this.lastAxisVertical;
      status.uiLastAxisStep = this.lastAxisStep;
      status.uiLastAxisNotches = this.lastAxisNotches;
      const runtime: RuntimeStatus = JSON.parse(status.runtimeJson) as RuntimeStatus;
      this.gameRunning = runtime.worldPlayerObserved === true && (runtime.presents || 0) > 0 && !runtime.error;
      const sampleMs: number = this.samplePerformance(runtime.presents || 0);
      this.memoryText = runtime.rssMB && runtime.rssMB > 0 ? `${runtime.rssMB.toFixed(0)} MB` : '—';
      const renderSize: number[] = runtime.spatialUpscale?.activeGameRenderSize || runtime.renderSize || [];
      const outputSize: number[] = runtime.spatialUpscale?.activeOutputSize || renderSize;
      this.resolutionText = renderSize.length === 2 ? `${renderSize[0]} × ${renderSize[1]}` : '—';
      this.outputResolutionText = outputSize.length === 2 ? `${outputSize[0]} × ${outputSize[1]}` : '—';
      status.uiPerformance = {
        visible: this.diagnosticsVisible, fps: this.fps, appCpuPercent: this.appCpuPercent,
        systemCpuPercent: this.systemCpuPercent, gpuLoadLevel: this.gpuLoadLevel,
        gpuFrequencyMHz: this.gpuFrequencyMHz, gpuMemoryMB: this.gpuMemoryMB, uptimeMs: sampleMs
      };
      if (runtime.worldPlayerObserved && (runtime.presents || 0) > 0 && !runtime.error) {
        gamePerformanceGameStarted();
      }
      if (!status.runtimeLinked) {
        this.statusText = '游戏运行时尚未链接';
      } else if (!status.surfaceReady) {
        this.statusText = '等待原生游戏窗口';
      } else if (!status.startDispatched) {
        this.statusText = status.width <= status.height ? '等待横屏游戏窗口' :
          `游戏启动请求失败 (${status.startResult})`;
      } else {
        if ((runtime.presents || 0) > 0 && !runtime.error) {
          this.statusText = this.gameRunning ? '游戏运行中' : '正在加载游戏世界';
        } else if (runtime.stage === 'checking-resources') {
          this.statusText = '检查游戏资源';
        } else if (runtime.stage === 'creating-vulkan-surface') {
          this.statusText = '创建游戏窗口';
        } else if (runtime.stage === 'loading-signed-game-aot') {
          this.statusText = '加载游戏程序';
        } else if (runtime.stage === 'running-game-main') {
          this.statusText = '游戏正在启动';
        } else if (runtime.stage === 'error') {
          this.statusText = '游戏启动遇到错误，请查看状态';
        } else {
          this.statusText = runtime.stage || runtime.status || '游戏运行时初始化中';
        }
      }
      const message: string = runtime.error || runtime.message || status.error || '';
      this.detailText = `${status.width} × ${status.height} · 输入 ${runtime.inputEvents || 0} · RSS ${runtime.rssMB || 0} MB` +
        ` · 游戏堆驻留 ${runtime.heapResidentMB || 0} MB / 虚拟 ${runtime.heapVirtualMB || 0} MB · ${message}`;
      const evidence: string = JSON.stringify(status);
      if (evidence !== this.lastEvidence) {
        this.lastEvidence = evidence;
        const file = fileIo.openSync(`${this.filesDirectory}/game-status.json`,
          fileIo.OpenMode.READ_WRITE | fileIo.OpenMode.CREATE | fileIo.OpenMode.TRUNC);
        try {
          fileIo.writeSync(file.fd, evidence);
        } finally {
          fileIo.closeSync(file);
        }
      }
    } catch (error) {
      this.statusText = '游戏状态读取失败';
      this.detailText = String(error);
    }
  }

  private handleKey(key: string, event: TouchEvent): void {
    if (event.type === TouchType.Down) {
      this.focusGame();
      game.setInputFocus(true);
      game.sendKey(key, true);
    } else if (event.type === TouchType.Up || event.type === TouchType.Cancel) {
      game.sendKey(key, false);
    }
  }

  private handleMouse(button: number, event: TouchEvent): void {
    if (event.type === TouchType.Down) {
      this.focusGame();
      game.setInputFocus(true);
      game.sendMouseButton(button, true);
    } else if (event.type === TouchType.Up || event.type === TouchType.Cancel) {
      game.sendMouseButton(button, false);
    }
  }

  @Builder
  private performancePanel() {
    Column({ space: 12 }) {
      Row({ space: 8 }) {
        Text('性能状态').fontSize(15).fontWeight(FontWeight.Medium).fontColor('#FFFFFF')
        Text(this.statusText).fontSize(11).fontColor('#B6C9D8')
        Blank()
        Text('v0.8.0').fontSize(11).fontColor('#B6C9D8')
      }.width('100%')
      Row({ space: 16 }) {
        Column({ space: 4 }) {
          Text('游戏帧率 · FPS').fontSize(11).fontColor('#B6C9D8')
          Text(this.fpsText).fontSize(20).fontWeight(FontWeight.Medium).fontColor('#FFFFFF')
        }.alignItems(HorizontalAlign.Start).layoutWeight(1)
        Column({ space: 4 }) {
          Text('CPU 占用 · 本应用').fontSize(11).fontColor('#B6C9D8')
          Text(this.appCpuText).fontSize(20).fontWeight(FontWeight.Medium).fontColor('#FFFFFF')
        }.alignItems(HorizontalAlign.Start).layoutWeight(1)
        Column({ space: 4 }) {
          Text('CPU 占用 · 系统').fontSize(11).fontColor('#B6C9D8')
          Text(this.systemCpuText).fontSize(20).fontWeight(FontWeight.Medium).fontColor('#FFFFFF')
        }.alignItems(HorizontalAlign.Start).layoutWeight(1)
        Column({ space: 4 }) {
          Text('GPU 负载 · 系统等级').fontSize(11).fontColor('#B6C9D8')
          Text(this.gpuLoadText).fontSize(20).fontWeight(FontWeight.Medium).fontColor('#FFFFFF')
        }.alignItems(HorizontalAlign.Start).layoutWeight(1)
      }.width('100%')
      Row({ space: 16 }) {
        Column({ space: 4 }) {
          Text('平均帧间隔').fontSize(11).fontColor('#B6C9D8')
          Text(this.frameTimeText).fontSize(20).fontWeight(FontWeight.Medium).fontColor('#FFFFFF')
        }.alignItems(HorizontalAlign.Start).layoutWeight(1)
        Column({ space: 4 }) {
          Text('内存 · 本应用').fontSize(11).fontColor('#B6C9D8')
          Text(this.memoryText).fontSize(20).fontWeight(FontWeight.Medium).fontColor('#FFFFFF')
        }.alignItems(HorizontalAlign.Start).layoutWeight(1)
        Column({ space: 4 }) {
          Text('显存 · 本应用').fontSize(11).fontColor('#B6C9D8')
          Text(this.gpuMemoryText).fontSize(20).fontWeight(FontWeight.Medium).fontColor('#FFFFFF')
        }.alignItems(HorizontalAlign.Start).layoutWeight(1)
        Column({ space: 4 }) {
          Text('GPU 频率').fontSize(11).fontColor('#B6C9D8')
          Text(this.gpuFrequencyText).fontSize(20).fontWeight(FontWeight.Medium).fontColor('#FFFFFF')
        }.alignItems(HorizontalAlign.Start).layoutWeight(1)
      }.width('100%')
      Row({ space: 12 }) {
        Text(`渲染 ${this.resolutionText}  →  输出 ${this.outputResolutionText}`)
          .fontSize(11).fontColor('#B6C9D8')
        Blank()
        Text('GPU 为 1–10 级负载；占用信息每 2 秒更新')
          .fontSize(10).fontColor('#90A7B9')
      }.width('100%')
    }.padding(14).width('100%').backgroundColor('#D9101820').borderRadius(14)
      .onMouse((event: MouseEvent) => { event.stopPropagation(); })
  }

  @Builder
  private mouseControl(label: string, button: number) {
    Text(label)
      .fontSize(15).fontWeight(FontWeight.Medium).fontColor('#FFFFFF')
      .width(54).height(48).textAlign(TextAlign.Center)
      .backgroundColor('#70344658').borderRadius(12)
      .border({ width: 1, color: '#80B4C8D8' })
      .onTouch((event: TouchEvent) => { this.handleMouse(button, event); })
      .onMouse((event: MouseEvent) => { event.stopPropagation(); })
  }

  @Builder
  private control(label: string, key: string) {
    Text(label)
      .fontSize(15).fontWeight(FontWeight.Medium).fontColor('#FFFFFF')
      .width(54).height(48).textAlign(TextAlign.Center)
      .backgroundColor('#70344658').borderRadius(12)
      .border({ width: 1, color: '#80B4C8D8' })
      .onTouch((event: TouchEvent) => { this.handleKey(key, event); })
      .onMouse((event: MouseEvent) => { event.stopPropagation(); })
  }

  @Builder
  private gameResourcePage() {
    Scroll() {
      Column({ space: 20 }) {
        Row() {
          Column({ space: 6 }) {
            Text($r('app.string.app_name')).fontSize(28).fontWeight(FontWeight.Bold).fontColor('#172C29')
            Text('导入自己的游戏资源，准备好后开始游戏。').fontSize(14).fontColor('#59726C')
          }.alignItems(HorizontalAlign.Start)
          Blank()
          Text('v0.8.0').fontSize(13).fontColor('#59726C')
        }.width('100%')

        Column({ space: 10 }) {
          Text(this.resourceChecking ? '检查资源中' : (this.resourceReady ? '资源已就绪' : '导入游戏资源'))
            .fontSize(20).fontWeight(FontWeight.Medium).fontColor('#183F34')
          Text(this.resourceMessage).fontSize(14).fontColor('#48665C').width('100%')
          if (!this.runtimeLinked) {
            Text('当前为公开应用壳。导入资源后，仍需安装自行编译并签名的完整游戏包才能运行游戏。')
              .fontSize(13).fontColor('#8B642A').width('100%')
          }
          Button(this.resourceChecking ? '正在检查资源…' : '开始游戏')
            .height(44).width('100%').backgroundColor('#23815F').fontColor('#FFFFFF')
            .enabled(this.resourceReady && this.runtimeLinked && !this.resourceChecking && !this.importing)
            .onClick(() => { this.beginGame(); })
        }.padding(20).width('100%').alignItems(HorizontalAlign.Start)
          .backgroundColor('#E4F1E9').borderRadius(18)

        Column({ space: 12 }) {
          Row() {
            Text('公共下载目录').fontSize(18).fontWeight(FontWeight.Medium).fontColor('#172C29')
            Blank()
            Button(this.packageListLoading ? '刷新中…' : '刷新').fontSize(13).height(32)
              .enabled(!this.packageListLoading && !this.importing)
              .onClick(() => { this.refreshPackages(); })
          }.width('100%')
          Text(this.directoryLabel).fontSize(14).fontColor('#23634B')
            .copyOption(CopyOptions.LocalDevice).width('100%')
          Text('用文件管理将游戏 ZIP 复制到这个目录，无需解压。回到这里刷新并点击文件导入。')
            .fontSize(13).fontColor('#59726C').width('100%')
          Row({ space: 12 }) {
            Button('打开下载目录').layoutWeight(1).height(40).fontSize(14)
              .enabled(this.directoryAvailable && !this.importing)
              .onClick(() => { this.openDownloadDirectory(); })
            Button('选择游戏 ZIP').layoutWeight(1).height(40).fontSize(14)
              .enabled(!this.importing && !this.resourceChecking)
              .onClick(() => { this.choosePackage(); })
          }.width('100%')
          if (this.packageList.length === 0) {
            Text('暂未找到 ZIP。复制后点击刷新，也可直接选择其他位置的文件。')
              .fontSize(13).fontColor('#798C84').padding({ top: 6, bottom: 6 }).width('100%')
          } else {
            ForEach(this.packageList, (item: DownloadGamePackage) => {
              Row({ space: 12 }) {
                Column({ space: 4 }) {
                  Text(item.name).fontSize(14).fontColor('#172C29').maxLines(2).textOverflow({ overflow: TextOverflow.Ellipsis })
                  Text(this.packageSize(item.bytes)).fontSize(12).fontColor('#798C84')
                }.layoutWeight(1).alignItems(HorizontalAlign.Start)
                Button('导入').height(36).fontSize(13)
                  .enabled(!this.importing && !this.resourceChecking)
                  .onClick(() => { this.importPackage(item.path, item.name); })
              }.padding(12).width('100%').backgroundColor('#F1F6F3').borderRadius(10)
            }, (item: DownloadGamePackage) => item.path)
          }
        }.padding(20).width('100%').alignItems(HorizontalAlign.Start)
          .backgroundColor('#FFFFFF').borderRadius(18)

        if (this.importing || this.importMessage.length > 0) {
          Column({ space: 10 }) {
            if (this.importFileName.length > 0) {
              Text(this.importFileName).fontSize(14).fontColor('#172C29').maxLines(2)
            }
            Text(this.importMessage).fontSize(14).fontColor('#48665C').width('100%')
            if (this.importing) {
              Progress({ value: this.importProgress, total: 100, type: ProgressType.Linear })
                .width('100%').color('#23815F')
              Text('请保持应用打开，等待解压和校验完成。').fontSize(12).fontColor('#798C84')
              Button('取消导入').height(36).fontSize(13).onClick(() => {
                this.importCancelRequested = true;
                game.cancelGameImport();
                this.importMessage = '正在取消，请等待临时资源清理完成…';
              })
            }
          }.padding(20).width('100%').alignItems(HorizontalAlign.Start)
            .backgroundColor('#FFFFFF').borderRadius(18)
        }
        Text('支持兼容的 ZIP 资源包和本地游戏备份包。原 ZIP 会保留，导入不会改动存档与个人设置。')
          .fontSize(12).fontColor('#798C84').width('100%')
      }.width('100%').constraintSize({ maxWidth: 920 }).padding(24).alignItems(HorizontalAlign.Start)
    }.width('100%').height('100%').backgroundColor('#F4F7F5')
  }

  build() {
    if (this.bufferProbeMode) {
      Column({ space: 16 }) {
        Text(this.probeTitle).fontSize(26).fontWeight(FontWeight.Bold).fontColor('#FFFFFF')
        Text(this.statusText).fontSize(20).fontColor('#D3E8F2')
        Text(`结果：${this.filesDirectory}/${this.probeResultFileName}`)
          .fontSize(13).fontColor('#A8BDD1').copyOption(CopyOptions.LocalDevice)
        Text('移除诊断请求文件后，重开应用可进入游戏。').fontSize(14).fontColor('#A8BDD1')
        Scroll() {
          Text(this.bufferProbeOutput).fontSize(13).fontColor('#EFF5FF').width('100%').copyOption(CopyOptions.LocalDevice)
        }.layoutWeight(1).width('100%')
      }.padding(32).width('100%').height('100%').backgroundColor('#101820')
    } else if (!this.gamePageVisible) {
      this.gameResourcePage()
    } else {
      Stack({ alignContent: Alignment.TopStart }) {
      XComponent({ id: 'muguet-game-surface', type: XComponentType.SURFACE, libraryname: 'harmony_probe' })
        .width('100%').height('100%')
        .focusable(false)
        .onAreaChange(() => { this.mousePositionKnown = false; })

      Column() {
        Row({ space: 8 }) {
          if (!this.gameRunning && !this.diagnosticsVisible) {
            Text(this.statusText).fontSize(12).fontColor('#FFFFFF')
              .padding(8).backgroundColor('#9A101820').borderRadius(8)
          }
          Blank()
          Button(this.mouseLook ? '鼠标：视角' : '鼠标：光标')
            .fontSize(12).height(32).focusable(false)
            .onMouse((event: MouseEvent) => { event.stopPropagation(); })
            .onClick(() => { this.toggleMouseLook(); })
          Button(this.controlsVisible ? '收起按键' : '显示按键')
            .fontSize(12).height(32).focusable(false)
            .onMouse((event: MouseEvent) => { event.stopPropagation(); })
            .onClick(() => { this.controlsVisible = !this.controlsVisible; })
          Button(this.diagnosticsVisible ? '收起状态' : '状态').fontSize(12).height(32).focusable(false)
            .onMouse((event: MouseEvent) => { event.stopPropagation(); })
            .onClick(() => { this.toggleStatus(); })
        }.width('100%')
        if (this.diagnosticsVisible) {
          this.performancePanel()
        }
        Blank()
        if (this.controlsVisible) {
          Row() {
            Column({ space: 4 }) {
              this.control('W ↑', 'KeyW')
              Row({ space: 4 }) {
                this.control('A ←', 'KeyA')
                this.control('S ↓', 'KeyS')
                this.control('D →', 'KeyD')
              }
              Row({ space: 4 }) {
                this.control('Shift', 'ShiftLeft')
                this.control('Esc', 'Escape')
              }
            }
            Blank()
            Column({ space: 4 }) {
              Row({ space: 4 }) {
                this.control('确认', 'Enter')
                this.control('上车 F', 'KeyF')
                this.control('跳跃', 'Space')
              }
              Row({ space: 4 }) {
                this.control('Q', 'KeyQ')
                this.control('E', 'KeyE')
                this.control('R', 'KeyR')
              }
              Row({ space: 4 }) {
                this.mouseControl('瞄准', 2)
                this.mouseControl('射击', 0)
                this.control('5', 'Digit5')
              }
              Text('滑动转动视角 · F8 切换鼠标光标').fontSize(11).fontColor('#D0E4EE')
            }
          }.width('100%').alignItems(VerticalAlign.Bottom)
        }
      }.width('100%').height('100%').padding({ left: 20, right: 20, top: 12, bottom: 16 })
        .hitTestBehavior(HitTestMode.Transparent)
      }.width('100%').height('100%').backgroundColor('#000000')
        .id('game-input').focusable(true).defaultFocus(true)
        .onFocus(() => { this.gameFocused = this.appForeground; game.setInputFocus(this.appForeground); })
        .onBlur(() => { this.gameFocused = false; this.resetInput(); })
        .onTouch((event: TouchEvent) => {
          if (event.type === TouchType.Down) { this.focusGame(); }
        })
        .onKeyEventDispatch((event: KeyEvent) => { return this.externalKey(event); })
        .onMouse((event: MouseEvent) => { this.externalMouse(event); })
        .onAxisEvent((event: AxisEvent) => { this.externalAxis(event); })
        .onHover((hovered: boolean) => {
          this.mousePositionKnown = false;
          this.showPointer(!hovered || !this.mouseLook);
          if (!hovered) { this.releaseMouseButtons(); }
        })
    }
  }
}
