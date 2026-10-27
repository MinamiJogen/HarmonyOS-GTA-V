import game, { GameStatus, VulkanBufferProbeRequest } from 'libharmony_probe.so';
import { common } from '@kit.AbilityKit';
import { fileIo } from '@kit.CoreFileKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import inputDevice from '@ohos.multimodalInput.inputDevice';
import pointer from '@ohos.multimodalInput.pointer';
import { systemDateTime } from '@kit.BasicServicesKit';
import { uiMaterial, window, ThemeControl, LengthMetrics } from '@kit.ArkUI';
import { FiveStarTouchControls } from '../controls/FiveStarTouchControls';
import { FiveStarSnapshot } from '../controls/FiveStarInput';
import { TouchSnapshot } from '../controls/TouchState';
import { GameKeyboard } from '../components/GameKeyboard';
import { gamePerformanceGameStarted, gamePerformanceNativeSample, NativeGameSample } from '../entryability/GamePerformance';
import { CpuSample, GpuSample, readCpuSample, readGpuSample } from '../telemetry/GameTelemetry';
import { LatestStatusWriter } from '../telemetry/LatestStatusWriter';
import { BenchmarkConditions, BenchmarkConditionsSnapshot, benchmarkMonitorEnabled } from '../telemetry/BenchmarkConditions';
import { applyGameDefaults } from '../config/GameDefaults';
import { GameQualitySelection, loadGameQuality, saveGameQuality, applyGameQuality, requireGameQualityCapabilities } from '../config/GameQuality';
import { selectGamePackage, selectSavePackage } from '../resources/PublicGamePackages';
import { GameSaveEntry, SavePickerDialog } from '../components/SavePickerDialog';
import { LauncherSectionCard } from '../components/LauncherSectionCard';
import { LauncherGraphicsSettings } from '../components/LauncherGraphicsSettings';
import { LauncherColors, launcherTheme } from '../theme/LauncherTheme';

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

interface GameSaveList {
  ok: boolean;
  error?: string;
  warning?: string;
  saves: GameSaveEntry[];
}

interface GameSaveImportResult extends GameSaveList {
  imported: number;
  bytes?: number;
  conflicts?: number;
  backupDirectory?: string;
  warning?: string;
}

interface SpatialStatus {
  activeGameRenderSize?: number[];
  activeOutputSize?: number[];
  mode?: string;
  nativeRecordedFrames?: number;
  nativeCommandsEncoded?: boolean;
  fallbackReason?: string;
}
interface FrameGenerationLive {
  state?: string;
  processId?: number;
  fresh?: boolean;
}

interface RuntimeStatus {
  status?: string;
  stage?: string;
  message?: string;
  error?: string;
  presents?: number;
  guestPresents?: number;
  nativeTelemetry?: NativeGameSample;
  frameGenerationAvailable?: boolean;
  frameGenerationLive?: FrameGenerationLive;
  presentationPaused?: boolean;
  inputEvents?: number;
  rssMB?: number;
  heapVirtualMB?: number;
  heapResidentMB?: number;
  worldPlayerObserved?: boolean;
  renderSize?: number[];
  spatialUpscale?: SpatialStatus;
  saveLoadPending?: boolean;
  saveLoadSucceeded?: boolean;
  saveLoadError?: string;
  sessionRestartId?: number;
  sessionRestartAction?: string;
  sessionRestartSaveId?: string;
  sessionRestartError?: string;
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
  uiStatusCallDurationMs?: number;
  uiLinxSyscap?: boolean;
  uiBenchmarkConditions?: BenchmarkConditionsSnapshot;
  uiPerformance?: PerformanceSnapshot;
  uiTouchState?: TouchSnapshot;
  uiVirtualPadState?: FiveStarSnapshot;
  uiToolbarExpanded?: boolean;
  uiKeyboardVisible?: boolean;
  uiTouchControlsVisible?: boolean;
  uiTouchInputTrace?: string;
  uiDetectedState?: string;
  uiSprintHeld?: boolean;
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
  @State private importing: boolean = false;
  @State private selectingPackage: boolean = false;
  @State private importProgress: number = 0;
  @State private importMessage: string = '';
  @State private importFileName: string = '';
  @State private saveBusy: boolean = false;
  @State private saveMessage: string = '';
  @State private saves: GameSaveEntry[] = [];
  @State private saveListVisible: boolean = false;
  @State private qualityPreset: string = 'medium';
  @State private superResolution: boolean = true;
  @State private frameGeneration: boolean = true;
  @State private qualityMessage: string = '';
  @State private capabilitiesKnown: boolean = false;
  @State private superResolutionAvailable: boolean = false;
  @State private frameGenerationAvailable: boolean = false;
  @State private capabilityMessage: string = '';
  @State private superResolutionStateText: string = '请求开启（待启动验证）';
  @State private frameGenerationStateText: string = '请求开启（待启动验证）';
  private lastSpatialFrames: number = -1;
  private lastSpatialProgressMs: number = -1;
  private saveDialogId: number = -1;
  private readonly controlMaterial: uiMaterial.ImmersiveMaterial = new uiMaterial.ImmersiveMaterial({
    style: uiMaterial.ImmersiveStyle.THIN, interactive: true, lightEffect: {}
  });
  private readonly gameMenuMaterial: uiMaterial.ImmersiveMaterial = new uiMaterial.ImmersiveMaterial({
    style: uiMaterial.ImmersiveStyle.THIN, materialColor: '#8024322D',
    interactive: true, lightEffect: { color: '#D6F5E6' }
  });
  private readonly dialogMaterial: uiMaterial.ImmersiveMaterial = new uiMaterial.ImmersiveMaterial({
    style: uiMaterial.ImmersiveStyle.ULTRA_THICK, materialColor: '#30DDEFE4'
  });
  private inputRegistered: boolean = false;
  private importPollTimer: number = -1;
  private importCancelRequested: boolean = false;
  @State private statusText: string = '等待原生游戏窗口';
  @State private detailText: string = '';
  @State private controlsVisible: boolean = true;
  @State private toolbarExpanded: boolean = false;
  @State private keyboardVisible: boolean = false;
  @State private childPanelOpen: boolean = false;
  @State private inputResetEpoch: number = 0;
  @State private closePanelsEpoch: number = 0;
  @State private touchStateJson: string = '{}';
  @State private virtualPadStateJson: string = '{}';
  @State private controllerConnected: boolean = false;
  @State private controlsSettingsEpoch: number = 0;
  @State private touchStatusText: string = '状态未确认';
  @State private gameWidth: number = 800;
  @State private gameHeight: number = 360;
  @State private gameAspect: number = 0;
  @State private safeLeft: number = 0;
  @State private safeRight: number = 0;
  @State private safeTop: number = 0;
  @State private safeBottom: number = 0;
  @State private exitConfirmVisible: boolean = false;
  private touchStateTimer: number = -1;
  private touchGeneration: number = -1;
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
  @State private uiCursorOwned: boolean = false;
  @StorageLink('gameAppForeground') @Watch('onAppForegroundChanged') private appForeground: boolean = false;
  @StorageLink('gameSessionRequestedSaveId') private requestedSessionSaveId: string = '';
  @StorageLink('gameSessionRequestedAction') @Watch('onRequestedSession') private requestedSessionAction: string = '';
  private handledSessionRestartId: number = 0;
  private sessionRestartFailure: string = '';
  @State private bufferProbeMode: boolean = false;
  @State private bufferProbeOutput: string = '';
  @State private probeTitle: string = 'Vulkan 缓冲区独立诊断';
  @State private probeResultFileName: string = 'vulkan-buffer-probe-result.json';
  private pollTimer: number = -1;
  private filesDirectory: string = '';
  private lastEvidence: string = '';
  private evidenceWriter: LatestStatusWriter = new LatestStatusWriter();
  private benchmarkConditions: BenchmarkConditions = new BenchmarkConditions();
  private linxSyscap: boolean = canIUse('SystemCapability.Commonlibrary.Linx');
  private lastSaveLoadError: string = '';
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
  private safeAreaWindow: window.Window | undefined = undefined;
  private safeAreaListener: () => void = () => {
    if (this.pageActive && this.gamePageVisible) {
      this.readSafeArea(true);
    }
  };
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
    this.evidenceWriter = new LatestStatusWriter();
    this.lastEvidence = '';
    ThemeControl.setDefaultTheme(launcherTheme);
    this.pageActive = true;
    const context = this.getUIContext().getHostContext() as common.UIAbilityContext;
    this.filesDirectory = context.filesDir;
    this.refreshProcessingCapabilities();
    try {
      const selection: GameQualitySelection = loadGameQuality(this.filesDirectory);
      this.qualityPreset = selection.preset;
      this.superResolution = selection.superResolution;
      this.frameGeneration = selection.frameGeneration;
    } catch (error) {
      this.qualityMessage = `读取画质配置失败：${String(error)}`;
    }
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
  }

  aboutToDisappear(): void {
    this.pageActive = false;
    this.benchmarkConditions.stop();
    this.evidenceWriter.stop();
    if (this.safeAreaWindow) {
      try { this.safeAreaWindow.off('avoidAreaChange', this.safeAreaListener); } catch (_) {}
      this.safeAreaWindow = undefined;
    }
    if (this.touchStateTimer >= 0) { clearInterval(this.touchStateTimer); this.touchStateTimer = -1; }
    this.closeSavePicker();
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
      this.resourceMessage = result.ready ? '' :
        '尚未找到完整资源。请在下方“文件导入”中导入游戏 ZIP。';
      if (!result.ready && result.error) {
        hilog.info(0x4100, 'GameImport', 'RESOURCE_CHECK %{public}s', result.error);
      }
    } catch (error) {
      if (this.pageActive) { this.resourceReady = false; this.resourceMessage = String(error); }
    } finally {
      if (this.pageActive) { this.resourceChecking = false; this.onRequestedSession(); }
    }
  }

  private onRequestedSession(): void {
    if (!this.pageActive || this.resourceChecking || !this.resourceReady || !this.runtimeLinked ||
      !this.requestedSessionAction || this.bufferProbeMode || this.importing || this.selectingPackage || this.saveBusy) { return; }
    const action: string = this.requestedSessionAction;
    const saveId: string = this.requestedSessionSaveId;
    this.requestedSessionAction = '';
    this.requestedSessionSaveId = '';
    if (this.gamePageVisible) { return; }
    if ((action !== 'load' && action !== 'new') || (action === 'new' && saveId !== '')) { return; }
    this.beginGame(action, saveId);
  }

  private continueOriginalSession(runtime: RuntimeStatus): void {
    const requestId: number = runtime.sessionRestartId || 0;
    if (requestId <= 0 || requestId === this.handledSessionRestartId || !this.pageActive) { return; }
    this.handledSessionRestartId = requestId;
    const action: string = runtime.sessionRestartAction || '';
    const saveId: string = runtime.sessionRestartSaveId || '';
    if (action !== 'load' || !/^userdata\/Documents\/Rockstar Games\/GTA V\/Default\/SGTA5[0-9]{4}$/.test(saveId) ||
      Number(saveId.slice(-4)) > 15) {
      this.sessionRestartFailure = '原游戏未提供有效的读档槽位，请退出游戏后选择存档。';
      return;
    }
    try {
      const context = this.getUIContext().getHostContext() as common.UIAbilityContext;
      this.resetInput();
      this.gameRunning = false;
      this.statusText = '正在重新载入所选存档';
      hilog.info(0x4100, 'GameSave', 'MENU_LOAD_RESTART id=%{public}d save=%{public}s', requestId, saveId);
      context.getApplicationContext().restartApp({
        bundleName: context.abilityInfo.bundleName, moduleName: 'entry', abilityName: 'EntryAbility',
        parameters: { muguetSessionAction: action, muguetSessionSaveId: saveId }
      });
    } catch (error) {
      this.sessionRestartFailure = `游戏内读档重启失败：${String(error)}。请退出游戏后重试。`;
      this.getUIContext().getPromptAction().showToast({ message: this.sessionRestartFailure, duration: 5000 });
    }
  }

  private async choosePackage(): Promise<void> {
    if (this.importing || this.selectingPackage || this.resourceChecking || this.gamePageVisible || this.saveBusy) { return; }
    this.selectingPackage = true;
    let uri: string = '';
    try {
      const context = this.getUIContext().getHostContext() as common.UIAbilityContext;
      uri = await selectGamePackage(context);
    } catch (error) {
      if (this.pageActive) { this.importMessage = `未选择文件或文件无法打开：${String(error)}`; }
    } finally {
      this.selectingPackage = false;
    }
    if (uri && this.pageActive) { await this.importPackage(uri, '所选游戏 ZIP'); }
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
    if (this.importing || this.resourceChecking || this.gamePageVisible || this.saveBusy) { return; }
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

  private async showSaves(): Promise<void> {
    if (this.saveBusy || this.importing || this.selectingPackage || this.gamePageVisible || this.saveListVisible) { return; }
    this.saveBusy = true;
    try {
      const result: GameSaveList = JSON.parse(await game.listGameSaves(this.filesDirectory)) as GameSaveList;
      if (!this.pageActive) { return; }
      if (!result.ok || result.error) { throw new Error(result.error || '无法读取存档列表'); }
      this.saves = result.saves.filter((item: GameSaveEntry) => item.mode === 1);
      if (result.warning) { this.saveMessage = result.warning; }
      if (this.saves.length === 0) {
        await this.getUIContext().getPromptAction().showDialog({
          title: '没有存档', message: '尚未找到可加载的剧情存档。可以先开始新游戏，或导入存档 ZIP。',
          buttons: [{ text: '知道了', color: $r('sys.color.font_emphasize') }],
          systemMaterial: this.dialogMaterial
        });
      } else {
        this.saveListVisible = true;
        const id: number = await this.getUIContext().getPromptAction().openCustomDialog({
          builder: () => { this.savePicker(); },
          alignment: DialogAlignment.Center,
          autoCancel: true,
          systemMaterial: this.dialogMaterial,
          onWillDismiss: (action: DismissDialogAction) => {
            this.saveListVisible = false;
            this.saveDialogId = -1;
            action.dismiss();
          }
        });
        this.saveDialogId = id;
        if (!this.pageActive || !this.saveListVisible || this.gamePageVisible) { this.closeSavePicker(); }
      }
    } catch (error) {
      this.saveListVisible = false;
      if (this.pageActive) { this.saveMessage = `读取存档失败：${String(error)}`; }
    } finally {
      if (this.pageActive) { this.saveBusy = false; }
    }
  }

  private closeSavePicker(): void {
    this.saveListVisible = false;
    const id: number = this.saveDialogId;
    this.saveDialogId = -1;
    if (id >= 0) {
      try {
        this.getUIContext().getPromptAction().closeCustomDialog(id);
      } catch (error) {
        hilog.warn(0x4100, 'GameSave', 'CLOSE_DIALOG_FAILED %{public}s', String(error));
      }
    }
  }

  private async chooseSavePackage(): Promise<void> {
    if (this.saveBusy || this.importing || this.selectingPackage || this.resourceChecking || this.gamePageVisible) { return; }
    let file: fileIo.File | undefined = undefined;
    this.saveBusy = true;
    this.saveMessage = '';
    try {
      const context = this.getUIContext().getHostContext() as common.UIAbilityContext;
      const uri: string = await selectSavePackage(context);
      if (!uri || !this.pageActive) { return; }
      file = await fileIo.open(uri, fileIo.OpenMode.READ_ONLY);
      this.saveMessage = '正在检查并导入存档…';
      const result: GameSaveImportResult = JSON.parse(await game.importGameSaves(file.fd, this.filesDirectory)) as GameSaveImportResult;
      if (!this.pageActive) { return; }
      if (!result.ok || result.error) {
        throw new Error((result.error || '存档导入失败') + (result.warning ? `；${result.warning}` : '') +
          (result.backupDirectory ? `；原档备份：${result.backupDirectory}` : ''));
      }
      this.saves = result.saves.filter((item: GameSaveEntry) => item.mode === 1);
      this.saveMessage = `已导入 ${result.imported} 个存档文件，可加载 ${this.saves.length} 个剧情进度。` +
        ((result.conflicts || 0) > 0 ? '同名旧档已备份。' : '') + (result.warning || '');
    } catch (error) {
      if (this.pageActive) { this.saveMessage = `导入存档失败：${String(error)}`; }
    } finally {
      if (file) { try { await fileIo.close(file); } catch (_) {} }
      if (this.pageActive) { this.saveBusy = false; }
    }
  }

  private async returnToLauncher(): Promise<void> {
    if (this.exitConfirmVisible) { return; }
    this.exitConfirmVisible = true;
    this.resetInput();
    try {
      const result = await this.getUIContext().getPromptAction().showDialog({
        title: '退出游戏', message: '退出当前游戏并返回启动页。请先确认游戏内保存已经完成，未保存的进度不会保留。',
        buttons: [{ text: '继续游戏', color: $r('sys.color.font_secondary') },
          { text: '退出游戏', color: $r('sys.color.font_emphasize') }],
        systemMaterial: this.dialogMaterial
      });
      if (result.index !== 1) { return; }
      const context = this.getUIContext().getHostContext() as common.UIAbilityContext;
      this.resetInput();
      await context.getApplicationContext().restartApp({
        bundleName: context.abilityInfo.bundleName, moduleName: 'entry', abilityName: 'EntryAbility'
      });
    } catch (error) {
      this.getUIContext().getPromptAction().showToast({ message: `退出游戏失败：${String(error)}` });
    } finally {
      this.exitConfirmVisible = false;
      this.mousePositionKnown = false;
      if (this.appForeground) { this.focusGame(); }
    }
  }

  onBackPress(): boolean {
    if (this.exitConfirmVisible) { return true; }
    if (this.saveListVisible) { this.closeSavePicker(); return true; }
    if (this.keyboardVisible) { this.toggleKeyboard(); return true; }
    if (this.childPanelOpen) { this.resetInput(); this.closePanelsEpoch += 1; return true; }
    if (this.diagnosticsVisible) { this.toggleStatus(); return true; }
    if (this.toolbarExpanded) { this.collapseAll(); return true; }
    if (this.gamePageVisible) { this.returnToLauncher(); return true; }
    return false;
  }

  private refreshProcessingCapabilities(): void {
    try {
      const capabilities = game.gameCapabilities();
      if (typeof capabilities.runtimeLinked !== 'boolean' ||
        typeof capabilities.superResolutionAvailable !== 'boolean' ||
        typeof capabilities.frameGenerationAvailable !== 'boolean') {
        throw new Error('原生能力查询返回无效数据');
      }
      this.runtimeLinked = capabilities.runtimeLinked;
      this.superResolutionAvailable = capabilities.superResolutionAvailable;
      this.frameGenerationAvailable = capabilities.frameGenerationAvailable;
      this.capabilitiesKnown = true;
      this.capabilityMessage = !capabilities.runtimeLinked ? '当前安装包未包含游戏运行时。' :
        (!capabilities.frameGenerationAvailable ? '本安装包未编译 AI 补帧。已保存请求保持不变；请明确关闭补帧后启动。' :
          '开关保存使用请求；超分接口、补帧模型和设备能力在启动后验证。');
    } catch (_) {
      this.capabilitiesKnown = false;
      this.superResolutionAvailable = false;
      this.frameGenerationAvailable = false;
      this.capabilityMessage = '无法确认当前安装包能力，请安装匹配的完整包。已保存选择保持不变。';
    }
  }

  private beginGame(action: string = 'demo', saveId: string = ''): void {
    if (this.importing || this.selectingPackage || this.resourceChecking || !this.resourceReady || !this.runtimeLinked || this.gamePageVisible || this.saveBusy) { return; }
    // Validate before either migration or quality application writes config.json.
    this.refreshProcessingCapabilities();
    if (!this.capabilitiesKnown) { this.qualityMessage = this.capabilityMessage; return; }
    try {
      requireGameQualityCapabilities({ preset: this.qualityPreset, superResolution: this.superResolution,
        frameGeneration: this.frameGeneration }, { runtimeLinked: this.runtimeLinked,
        superResolutionAvailable: this.superResolutionAvailable, frameGenerationAvailable: this.frameGenerationAvailable });
    } catch (error) {
      this.qualityMessage = String(error);
      this.getUIContext().getPromptAction().showToast({ message: this.qualityMessage, duration: 5000 });
      return;
    }
    try {
      applyGameDefaults(this.filesDirectory);
      applyGameQuality(this.filesDirectory, {
        preset: this.qualityPreset, superResolution: this.superResolution, frameGeneration: this.frameGeneration
      });
      // Read this optional diagnostic flag once at game startup, never on the
      // status/input polling paths. Missing flags keep the monitor off.
      this.benchmarkConditions.configure(benchmarkMonitorEnabled(
        fileIo.readTextSync(`${this.filesDirectory}/config.json`)));
      game.configureGame(this.filesDirectory, action, saveId, this.frameGeneration);
      this.closeSavePicker();
      inputDevice.on('change', this.inputListener);
      this.inputRegistered = true;
      this.gamePageVisible = true;
      this.benchmarkConditions.setActive(this.pageActive && this.appForeground);
      this.readSafeArea();
      this.touchStateTimer = setInterval(() => { this.refreshTouchState(); }, 50);
      this.refreshStatus();
      this.pollTimer = setInterval(() => { this.refreshStatus(); }, 1000);
    } catch (error) {
      this.benchmarkConditions.stop();
      if (this.inputRegistered) { inputDevice.off('change', this.inputListener); this.inputRegistered = false; }
      game.stopGame();
      this.resourceMessage = `游戏初始化失败：${String(error)}`;
      hilog.error(0x4100, 'GameImport', 'START_FAILED %{public}s', String(error));
    }
  }

  private selectQuality(preset: string, superResolution: boolean, frameGeneration: boolean): boolean {
    if (this.gamePageVisible || this.importing || this.selectingPackage || this.saveBusy) { return false; }
    try {
      if ((frameGeneration && !this.frameGeneration && (!this.capabilitiesKnown || !this.frameGenerationAvailable)) ||
        (superResolution && !this.superResolution && (!this.capabilitiesKnown || !this.superResolutionAvailable))) {
        this.qualityMessage = this.capabilityMessage;
        return false;
      }
      saveGameQuality(this.filesDirectory, { preset, superResolution, frameGeneration });
      this.qualityPreset = preset;
      this.superResolution = superResolution;
      this.frameGeneration = frameGeneration;
      this.qualityMessage = '';
      return true;
    } catch (error) {
      this.qualityMessage = `画质选择未保存：${String(error)}`;
      return false;
    }
  }

  private focusGame(): void {
    if (!this.appForeground || this.exitConfirmVisible) { return; }
    game.setInputFocus(true);
    this.getUIContext().getFocusController().requestFocus('game-input');
  }

  private onAppForegroundChanged(): void {
    if (!this.gamePageVisible) { return; }
    this.benchmarkConditions.setActive(this.pageActive && this.appForeground);
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
      this.readSafeArea();
      game.setInputFocus(true);
      this.focusGame();
    } else {
      this.touchStateJson = '{}';
      this.gameFocused = false;
    }
  }

  private toggleStatus(): void {
    this.resetInput();
    this.keyboardVisible = false;
    this.closePanelsEpoch += 1;
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
    if (!this.diagnosticsVisible) { this.focusGame(); }
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
    this.inputResetEpoch += 1;
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
    this.resetInput();
    this.mouseLook = !this.mouseLook;
    this.mousePositionKnown = false;
    this.showPointer(!this.effectiveMouseLook());
    this.focusGame();
  }

  private toggleKeyboard(): void {
    this.resetInput();
    this.closePanelsEpoch += 1;
    this.keyboardVisible = !this.keyboardVisible;
    this.diagnosticsVisible = false;
    this.mousePositionKnown = false;
    this.focusGame();
  }

  private collapseAll(): void {
    this.resetInput();
    this.toolbarExpanded = false;
    this.keyboardVisible = false;
    this.diagnosticsVisible = false;
    this.childPanelOpen = false;
    this.closePanelsEpoch += 1;
    this.telemetryEpoch += 1;
    this.focusGame();
  }

  private refreshTouchState(): void {
    if (!this.pageActive || !this.appForeground || !this.gamePageVisible) { return; }
    let cursor: boolean = false;
    let changedGeneration: boolean = false;
    try {
      this.touchStateJson = game.getTouchState();
      this.virtualPadStateJson = game.virtualPadState();
      this.controllerConnected = game.physicalControllerConnected();
      const state: TouchSnapshot = JSON.parse(this.touchStateJson) as TouchSnapshot;
      if (Number.isFinite(state.generation)) {
        changedGeneration = this.touchGeneration >= 0 && this.touchGeneration !== state.generation;
        this.touchGeneration = state.generation;
      }
      const ui = state.ui;
      const owner = state.uiOwnsInput;
      cursor = ui?.known === true && owner?.known === true && owner.value === true &&
        ui.generation === state.generation && owner.generation === state.generation &&
        ui.sampledAtMonoMs !== undefined && owner.sampledAtMonoMs !== undefined &&
        state.nowMonoMs - ui.sampledAtMonoMs <= 500 && state.nowMonoMs - owner.sampledAtMonoMs <= 500 &&
        ['pause', 'map', 'phone', 'phoneCamera', 'web', 'startup'].includes(ui.value || '');
      const virtualState: FiveStarSnapshot = JSON.parse(this.virtualPadStateJson) as FiveStarSnapshot;
      if (virtualState.hookReady === true && (virtualState.ageMs || 0) <= 1500) {
        cursor = cursor || virtualState.pauseMenu || (virtualState.status & 2) !== 0 || virtualState.hints?.phoneBrowser === true;
      }
    } catch (_) { this.touchStateJson = '{}'; }
    if (cursor !== this.uiCursorOwned || changedGeneration) {
      this.resetInput(); this.uiCursorOwned = cursor; this.showPointer(!this.effectiveMouseLook());
      this.focusGame();
    }
  }

  private effectiveMouseLook(): boolean { return this.mouseLook && !this.uiCursorOwned && !this.keyboardVisible; }

  private async readSafeArea(restoreInput: boolean = false): Promise<void> {
    try {
      const context = this.getUIContext().getHostContext() as common.UIAbilityContext;
      const mainWindow = await window.getLastWindow(context);
      if (!this.pageActive || !this.gamePageVisible) { return; }
      if (!this.safeAreaWindow) {
        mainWindow.on('avoidAreaChange', this.safeAreaListener);
        this.safeAreaWindow = mainWindow;
      }
      const cutout: window.AvoidArea = mainWindow.getWindowAvoidArea(window.AvoidAreaType.TYPE_CUTOUT);
      const system: window.AvoidArea = mainWindow.getWindowAvoidArea(window.AvoidAreaType.TYPE_SYSTEM);
      const navigation: window.AvoidArea = mainWindow.getWindowAvoidArea(window.AvoidAreaType.TYPE_NAVIGATION_INDICATOR);
      const left: number = this.getUIContext().px2vp(Math.max(cutout.leftRect.width, system.leftRect.width));
      const right: number = this.getUIContext().px2vp(Math.max(cutout.rightRect.width, system.rightRect.width));
      const top: number = this.getUIContext().px2vp(Math.max(cutout.topRect.height, system.topRect.height));
      const bottom: number = this.getUIContext().px2vp(Math.max(cutout.bottomRect.height, system.bottomRect.height, navigation.bottomRect.height));
      if (left !== this.safeLeft || right !== this.safeRight || top !== this.safeTop || bottom !== this.safeBottom) {
        if (restoreInput) { this.resetInput(); }
        this.safeLeft = left; this.safeRight = right; this.safeTop = top; this.safeBottom = bottom;
        if (restoreInput && !this.diagnosticsVisible && !this.keyboardVisible && !this.childPanelOpen) { this.focusGame(); }
      }
    } catch (_) {}
  }

  private externalKey(event: KeyEvent): boolean {
    this.lastKeyCode = event.keyCode;
    if (!this.appForeground || this.exitConfirmVisible) {
      if (event.type === KeyType.Up) { game.sendKeyEvent(event.keyCode, false, 0, true); }
      return false;
    }
    if (event.type !== KeyType.Down && event.type !== KeyType.Up) { return false; }
    // GameControllerKit already publishes these events as one native ioPad
    // sample. Do not also send its compatibility D-pad keys as PC arrows.
    if (event.keySource === KeySource.JOYSTICK && this.controllerConnected) { return true; }
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
      game.sendPointer(this.getUIContext().vp2px(event.x), this.getUIContext().vp2px(event.y), 0, 0, this.effectiveMouseLook());
    }
    if (event.action === MouseAction.Move) {
      const rawX: number = event.rawDeltaX || 0;
      const rawY: number = event.rawDeltaY || 0;
      const useRaw: boolean = rawX !== 0 || rawY !== 0;
      const dx: number = useRaw ? rawX : (this.mousePositionKnown ? event.x - this.lastMouseX : 0);
      const dy: number = useRaw ? rawY : (this.mousePositionKnown ? event.y - this.lastMouseY : 0);
      game.sendPointer(this.getUIContext().vp2px(event.x), this.getUIContext().vp2px(event.y), dx, dy, this.effectiveMouseLook());
      this.showPointer(!this.effectiveMouseLook());
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
      const statusBegin: number = systemDateTime.getUptime(systemDateTime.TimeType.STARTUP, false);
      const status: UiGameStatus = game.gameStatus() as UiGameStatus;
      status.uiStatusCallDurationMs = systemDateTime.getUptime(systemDateTime.TimeType.STARTUP, false) - statusBegin;
      status.uiLinxSyscap = this.linxSyscap;
      status.uiTouchState = JSON.parse(this.touchStateJson) as TouchSnapshot;
      status.uiVirtualPadState = JSON.parse(this.virtualPadStateJson) as FiveStarSnapshot;
      status.uiToolbarExpanded = this.toolbarExpanded;
      status.uiKeyboardVisible = this.keyboardVisible;
      status.uiTouchControlsVisible = this.controlsVisible;
      status.uiTouchInputTrace = AppStorage.get<string>('gameTouchInputTrace') || '';
      status.uiDetectedState = this.touchStatusText;
      status.uiSprintHeld = AppStorage.get<boolean>('gameSprintHeld') || false;
      status.uiAppForeground = this.appForeground;
      status.uiGameFocused = this.gameFocused;
      status.uiMouseLook = this.mouseLook;
      status.uiLastKeyCode = this.lastKeyCode;
      status.uiLastAxisVertical = this.lastAxisVertical;
      status.uiLastAxisStep = this.lastAxisStep;
      status.uiLastAxisNotches = this.lastAxisNotches;
      const runtime: RuntimeStatus = JSON.parse(status.runtimeJson) as RuntimeStatus;
      // Only read this process's native status. Never open a prior FG status file.
      const processingNow: number = systemDateTime.getUptime(systemDateTime.TimeType.STARTUP, false);
      this.benchmarkConditions.setActive(this.pageActive && this.appForeground && this.gamePageVisible &&
        !runtime.error && runtime.status !== 'stopped' && runtime.stage !== 'stopped');
      status.uiBenchmarkConditions = this.benchmarkConditions.snapshot(processingNow);
      const srFrames: number = runtime.spatialUpscale?.nativeRecordedFrames ?? 0;
      if (srFrames > this.lastSpatialFrames && srFrames > 0) { this.lastSpatialProgressMs = processingNow; }
      this.lastSpatialFrames = srFrames;
      this.superResolutionStateText = !this.superResolution ? '已关闭' :
        (!this.superResolutionAvailable ? '不可用（当前安装包）' :
          (runtime.spatialUpscale?.fallbackReason ? `不可用：${runtime.spatialUpscale.fallbackReason}` :
            (runtime.presentationPaused ? '已暂停' :
              (runtime.spatialUpscale?.nativeCommandsEncoded === true && runtime.spatialUpscale?.mode !== 'off' &&
                this.lastSpatialProgressMs >= 0 && processingNow - this.lastSpatialProgressMs <= 5000 ?
                '运行中' : '请求开启（等待超分输出）'))));
      const fgState: string = runtime.frameGenerationLive?.state || '';
      this.frameGenerationStateText = !this.frameGeneration ? '已关闭' :
        (runtime.frameGenerationAvailable === false ? '不可用（安装包未编译）' :
          (fgState === 'running' && runtime.frameGenerationLive?.fresh === true ? '运行中（已呈现新 AI 帧）' :
            (fgState === 'unavailable_runtime_error' ? '不可用（初始化或执行失败，已回退原帧）' :
              (fgState === 'paused' ? '已暂停' : (fgState === 'stopped' ? '已停止' :
                (fgState === 'unavailable_not_compiled' ? '不可用（安装包未编译）' : '请求开启（等待新 AI 输出）'))))));
      if (runtime.saveLoadError && runtime.saveLoadError !== this.lastSaveLoadError) {
        this.lastSaveLoadError = runtime.saveLoadError;
        this.getUIContext().getPromptAction().showToast({ message: runtime.saveLoadError, duration: 5000 });
      }
      this.gameRunning = runtime.worldPlayerObserved === true && (runtime.presents || 0) > 0 && !runtime.error && !runtime.saveLoadPending && !runtime.saveLoadError;
      if (this.sessionRestartFailure || runtime.sessionRestartError) { this.gameRunning = false; }
      const sampleMs: number = this.samplePerformance(runtime.guestPresents || 0);
      this.memoryText = runtime.rssMB && runtime.rssMB > 0 ? `${runtime.rssMB.toFixed(0)} MB` : '—';
      const renderSize: number[] = runtime.spatialUpscale?.activeGameRenderSize || runtime.renderSize || [];
      const outputSize: number[] = runtime.spatialUpscale?.activeOutputSize || renderSize;
      if (renderSize.length === 2 && renderSize[1] > 0) { this.gameAspect = renderSize[0] / renderSize[1]; }
      this.resolutionText = renderSize.length === 2 ? `${renderSize[0]} × ${renderSize[1]}` : '—';
      this.outputResolutionText = outputSize.length === 2 ? `${outputSize[0]} × ${outputSize[1]}` : '—';
      status.uiPerformance = {
        visible: this.diagnosticsVisible, fps: this.fps, appCpuPercent: this.appCpuPercent,
        systemCpuPercent: this.systemCpuPercent, gpuLoadLevel: this.gpuLoadLevel,
        gpuFrequencyMHz: this.gpuFrequencyMHz, gpuMemoryMB: this.gpuMemoryMB, uptimeMs: sampleMs
      };
      if (runtime.worldPlayerObserved && (runtime.presents || 0) > 0 && !runtime.error) {
        gamePerformanceGameStarted();
        if (runtime.nativeTelemetry) {
          gamePerformanceNativeSample(runtime.nativeTelemetry,
            runtime.nativeTelemetry.sourceTargetFps, runtime.nativeTelemetry.sourceTargetFps);
        }
      }
      if (!status.runtimeLinked) {
        this.statusText = '游戏运行时尚未链接';
      } else if (!status.surfaceReady) {
        this.statusText = '等待原生游戏窗口';
      } else if (!status.startDispatched) {
        this.statusText = status.width <= status.height ? '等待横屏游戏窗口' :
          `游戏启动请求失败 (${status.startResult})`;
      } else {
        if (this.sessionRestartFailure || runtime.sessionRestartError) {
          this.statusText = '游戏内读档未完成，请退出游戏后重试';
        } else if (runtime.saveLoadError) {
          this.statusText = '存档加载失败，请退出游戏后重试';
        } else if (runtime.saveLoadPending) {
          this.statusText = '正在加载所选存档';
        } else if ((runtime.presents || 0) > 0 && !runtime.error) {
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
          this.statusText = '游戏启动遇到错误，请打开性能检测';
        } else {
          this.statusText = runtime.stage || runtime.status || '游戏运行时初始化中';
        }
      }
      const message: string = this.sessionRestartFailure || runtime.sessionRestartError || runtime.saveLoadError || runtime.error || runtime.message || status.error || '';
      this.detailText = `${status.width} × ${status.height} · 输入 ${runtime.inputEvents || 0} · RSS ${runtime.rssMB || 0} MB` +
        ` · 游戏堆驻留 ${runtime.heapResidentMB || 0} MB / 虚拟 ${runtime.heapVirtualMB || 0} MB · ${message}`;
      const evidence: string = JSON.stringify(status);
      if (evidence !== this.lastEvidence) {
        this.lastEvidence = evidence;
        this.evidenceWriter.enqueue(this.filesDirectory, evidence);
      }
      this.continueOriginalSession(runtime);
    } catch (error) {
      this.statusText = '游戏状态读取失败';
      this.detailText = String(error);
    }
  }

  @Builder
  private toolbarItem(label: string, active: boolean, action: () => void) {
    Button(label, { type: ButtonType.Capsule, buttonStyle: ButtonStyleMode.NORMAL })
      .width(label.length * 12 + 24).height(44).fontSize(12).focusable(false)
      .padding({ left: 12, right: 12 })
      .fontColor(active ? '#B8F2D7' : '#F4F8F6')
      .systemMaterial(this.gameMenuMaterial)
      .accessibilitySelected(active)
      .onMouse((event: MouseEvent) => { event.stopPropagation(); })
      .onTouch((event: TouchEvent) => { event.stopPropagation(); })
      .onClick(action)
  }

  @Builder
  private menuToggle() {
    Button(this.toolbarExpanded ? '›' : '≡', { type: ButtonType.Circle, buttonStyle: ButtonStyleMode.NORMAL })
      .width(32).height(32).fontSize(23).focusable(false).margin(6)
      .responseRegion({ x: -6, y: -6, width: 44, height: 44 })
      .fontColor('#F4F8F6').systemMaterial(this.gameMenuMaterial)
      .accessibilityText(this.toolbarExpanded ? '收起游戏菜单' : '展开游戏菜单')
      .onMouse((event: MouseEvent) => { event.stopPropagation(); })
      .onTouch((event: TouchEvent) => { event.stopPropagation(); })
      .onClick(() => {
        if (this.toolbarExpanded) { this.collapseAll(); }
        else { this.resetInput(); this.toolbarExpanded = true; this.closePanelsEpoch += 1; }
      })
  }

  @Builder
  private gameToolbar() {
    Row({ space: 6 }) {
      if (this.toolbarExpanded) {
        Flex({ direction: FlexDirection.Row, wrap: FlexWrap.Wrap, justifyContent: FlexAlign.End,
          space: { main: LengthMetrics.vp(6), cross: LengthMetrics.vp(6) } }) {
          this.toolbarItem('退出游戏', false, () => { this.returnToLauncher(); })
          this.toolbarItem(this.mouseLook ? '鼠标：视角' : '鼠标：光标', false, () => { this.toggleMouseLook(); })
          this.toolbarItem('全键盘按键', this.keyboardVisible, () => { this.toggleKeyboard(); })
          this.toolbarItem('触控设置', false, () => {
            this.toolbarExpanded = false; this.controlsVisible = true; this.controlsSettingsEpoch += 1;
          })
          this.toolbarItem(this.controlsVisible ? '收起按键' : '显示按键', !this.controlsVisible, () => {
            this.resetInput(); this.closePanelsEpoch += 1; this.controlsVisible = !this.controlsVisible;
          })
          this.toolbarItem('性能检测', this.diagnosticsVisible, () => { this.toggleStatus(); })
        }.width(Math.min(408, Math.max(180, this.gameWidth - this.safeLeft - this.safeRight - 92)))
      }
      this.menuToggle()
    }.alignItems(VerticalAlign.Top)
      .padding(this.toolbarExpanded ? { left: 8, right: 2, top: 6, bottom: 6 } : 0)
      .borderRadius(28)
      .backgroundColor(this.toolbarExpanded ? '#CE182522' : '#00000000')
      .onMouse((event: MouseEvent) => { event.stopPropagation(); })
      .onTouch((event: TouchEvent) => { event.stopPropagation(); })
  }

  @Builder
  private metric(label: string, value: string) {
    Column({ space: 5 }) {
      Text(label).fontSize(10).fontColor('#A6BBB2')
      Text(value).fontSize(18).fontWeight(FontWeight.Medium).fontColor('#F4FAF6')
        .maxLines(1).textOverflow({ overflow: TextOverflow.Ellipsis })
    }.alignItems(HorizontalAlign.Start).layoutWeight(1).padding(10)
      .backgroundColor('#402E4240').borderRadius(12)
  }

  @Builder
  private performancePanel() {
    Column({ space: 10 }) {
      Row({ space: 8 }) {
        Column({ space: 3 }) {
          Text('性能检测').fontSize(17).fontWeight(FontWeight.Bold).fontColor('#F4FAF6')
          Text(this.statusText).fontSize(10).fontColor('#A6BBB2').maxLines(1)
          Text(this.touchStatusText).fontSize(11).fontColor('#B5EAD3').maxLines(1)
        }.alignItems(HorizontalAlign.Start).layoutWeight(1)
        Text('实时').fontSize(10).fontColor('#B5EAD3').padding({ left: 8, right: 8, top: 4, bottom: 4 })
          .backgroundColor('#254F7964').borderRadius(12)
        Button('×', { type: ButtonType.Circle }).width(44).height(44).fontSize(23).focusable(false)
          .fontColor('#DDEBE3').backgroundColor('#30455650').accessibilityText('关闭性能检测')
          .onClick(() => { this.toggleStatus(); })
      }.width('100%')
      Scroll() {
        Column({ space: 8 }) {
          Row({ space: 12 }) {
            Column({ space: 1 }) {
              Row({ space: 6 }) {
                Text(this.fpsText).fontSize(40).fontWeight(FontWeight.Bold).fontColor('#B5EAD3')
                Text('FPS').fontSize(12).fontColor('#A6BBB2').margin({ top: 18 })
              }
              Text('游戏原生帧率').fontSize(10).fontColor('#A6BBB2')
            }.alignItems(HorizontalAlign.Start).layoutWeight(1)
            Column({ space: 5 }) {
              Text('平均帧间隔').fontSize(10).fontColor('#A6BBB2')
              Text(this.frameTimeText).fontSize(20).fontWeight(FontWeight.Medium).fontColor('#F4FAF6')
            }.alignItems(HorizontalAlign.End)
          }.width('100%').padding(12).backgroundColor('#304B745F').borderRadius(14)
          Row({ space: 8 }) { this.metric('CPU · 本应用', this.appCpuText); this.metric('CPU · 系统', this.systemCpuText) }.width('100%')
          Row({ space: 8 }) { this.metric('GPU · 系统负载等级', this.gpuLoadText); this.metric('GPU · 频率', this.gpuFrequencyText) }.width('100%')
          Row({ space: 8 }) { this.metric('内存 · 本应用', this.memoryText); this.metric('显存 · 本应用', this.gpuMemoryText) }.width('100%')
          Column({ space: 4 }) {
            Text(`渲染  ${this.resolutionText}`).fontSize(11).fontColor('#CCDAD3')
            Text(`输出  ${this.outputResolutionText}`).fontSize(11).fontColor('#CCDAD3')
            Text(`超分  ${this.superResolutionStateText}`).fontSize(11).fontColor('#CCDAD3')
            Text(`补帧  ${this.frameGenerationStateText}`).fontSize(11).fontColor('#CCDAD3')
            Text('GPU 为系统 1–10 级负载，非百分比；占用每 2 秒采样。').fontSize(10).fontColor('#8BA69A')
            Text('FPS 不计 AI 补帧和重复帧；平均间隔并非 GPU 渲染耗时。').fontSize(10).fontColor('#8BA69A')
          }.width('100%').alignItems(HorizontalAlign.Start).padding({ top: 4, bottom: 4 })
        }.width('100%')
      }.width('100%').layoutWeight(1).scrollBar(BarState.Auto)
    }.padding(14).width(Math.min(420, this.gameWidth - this.safeLeft - this.safeRight - 24))
      .height(Math.min(410, this.gameHeight - this.safeTop - this.safeBottom - (this.gameWidth < 700 ? 128 : 80)))
      .backgroundColor('#F2182528').borderRadius(22).border({ width: 1, color: '#35C5E0D1' })
      .shadow({ radius: 24, color: '#70000000', offsetY: 8 })
      .onMouse((event: MouseEvent) => { event.stopPropagation(); })
      .onTouch((event: TouchEvent) => { event.stopPropagation(); })
  }

  @Builder
  private resourceAndStartupContent() {
    Text(this.resourceChecking ? '检查资源中' : (this.resourceReady ? '资源已就绪' : '等待导入游戏资源'))
      .fontSize(16).fontWeight(FontWeight.Medium).fontColor(LauncherColors.accent)
    if (this.resourceMessage.length > 0) {
      Text(this.resourceMessage).fontSize(14).fontColor(LauncherColors.secondaryText).width('100%')
    }
    if (!this.runtimeLinked) {
      Text('当前为公开应用壳。导入资源后，仍需安装自行编译并签名的完整游戏包才能运行游戏。')
        .fontSize(13).fontColor(LauncherColors.secondaryText).width('100%')
    }
    LauncherGraphicsSettings({
      preset: this.qualityPreset,
      superResolution: this.superResolution,
      frameGeneration: this.frameGeneration,
      capabilitiesKnown: this.capabilitiesKnown,
      superResolutionAvailable: this.superResolutionAvailable,
      frameGenerationAvailable: this.frameGenerationAvailable,
      settingsEnabled: !this.importing && !this.selectingPackage && !this.saveBusy && !this.resourceChecking,
      onPresetChange: (preset: string) => {
        return this.selectQuality(preset, this.superResolution, this.frameGeneration);
      },
      onSuperResolutionChange: (enabled: boolean) => {
        return this.selectQuality(this.qualityPreset, enabled, this.frameGeneration);
      },
      onFrameGenerationChange: (enabled: boolean) => {
        return this.selectQuality(this.qualityPreset, this.superResolution, enabled);
      }
    })
    Text(this.capabilityMessage).fontSize(12).fontColor(LauncherColors.secondaryText).width('100%')
    if (this.qualityMessage.length > 0) {
      Text(this.qualityMessage).fontSize(12).fontColor(LauncherColors.accent).width('100%')
    }
    Row({ space: 12 }) {
      Button('开始新游戏').layoutWeight(1).constraintSize({ minHeight: 48 })
        .buttonStyle(ButtonStyleMode.EMPHASIZED)
        .enabled(this.resourceReady && this.runtimeLinked && !this.resourceChecking && !this.importing && !this.selectingPackage && !this.saveBusy)
        .onClick(() => { this.beginGame('new'); })
      Button('加载存档').layoutWeight(1).constraintSize({ minHeight: 48 })
        .buttonStyle(ButtonStyleMode.NORMAL)
        .backgroundColor(LauncherColors.secondaryButton).fontColor(LauncherColors.accent)
        .border({ width: 1, color: LauncherColors.secondaryButtonBorder })
        .enabled(this.resourceReady && this.runtimeLinked && !this.resourceChecking && !this.importing && !this.selectingPackage && !this.saveBusy)
        .onClick(() => { this.showSaves(); })
      Button('开始 Demo').layoutWeight(1).constraintSize({ minHeight: 48 })
        .buttonStyle(ButtonStyleMode.NORMAL)
        .backgroundColor(LauncherColors.secondaryButton).fontColor(LauncherColors.accent)
        .border({ width: 1, color: LauncherColors.secondaryButtonBorder })
        .enabled(this.resourceReady && this.runtimeLinked && !this.resourceChecking && !this.importing && !this.selectingPackage && !this.saveBusy)
        .onClick(() => { this.beginGame('demo'); })
    }.width('100%')
  }

  @Builder
  private importFilesContent() {
    Text('选择 ZIP 文件直接导入，无需提前解压。')
      .fontSize(14).fontColor(LauncherColors.secondaryText).width('100%')
    Row({ space: 12 }) {
      Button('导入游戏 ZIP').layoutWeight(1).constraintSize({ minHeight: 48 })
        .buttonStyle(ButtonStyleMode.NORMAL)
        .backgroundColor(LauncherColors.secondaryButton).fontColor(LauncherColors.accent)
        .border({ width: 1, color: LauncherColors.secondaryButtonBorder })
        .enabled(!this.importing && !this.selectingPackage && !this.resourceChecking && !this.saveBusy)
        .onClick(() => { this.choosePackage(); })
      Button('导入存档 ZIP').layoutWeight(1).constraintSize({ minHeight: 48 })
        .buttonStyle(ButtonStyleMode.NORMAL)
        .backgroundColor(LauncherColors.secondaryButton).fontColor(LauncherColors.accent)
        .border({ width: 1, color: LauncherColors.secondaryButtonBorder })
        .enabled(!this.saveBusy && !this.importing && !this.selectingPackage && !this.resourceChecking)
        .onClick(() => { this.chooseSavePackage(); })
    }.width('100%')
    if (this.saveBusy) {
      Row({ space: 8 }) {
        LoadingProgress().width(24).height(24).color(LauncherColors.accent)
        Text('正在处理存档…').fontSize(14).fontColor(LauncherColors.secondaryText)
      }.width('100%')
    }
    if (this.saveMessage.length > 0) {
      Text(this.saveMessage).fontSize(13).fontColor(LauncherColors.secondaryText).width('100%')
    }
    if (this.importing || this.importMessage.length > 0) {
      Divider().color(LauncherColors.border)
      if (this.importFileName.length > 0) {
        Text(this.importFileName).fontSize(14).fontColor(LauncherColors.text).maxLines(2)
      }
      Text(this.importMessage).fontSize(14).fontColor(LauncherColors.secondaryText).width('100%')
      if (this.importing) {
        Progress({ value: this.importProgress, total: 100, type: ProgressType.Linear })
          .width('100%').color(LauncherColors.accent)
        Text('请保持应用打开，等待解压和校验完成。').fontSize(12).fontColor(LauncherColors.secondaryText)
        Button('取消导入').constraintSize({ minHeight: 48 }).buttonStyle(ButtonStyleMode.TEXTUAL).onClick(() => {
          this.importCancelRequested = true;
          game.cancelGameImport();
          this.importMessage = '正在取消，请等待临时资源清理完成…';
        })
      }
    }
  }

  @Builder
  private gameResourcePage() {
    WithTheme({ theme: launcherTheme, colorMode: ThemeColorMode.LIGHT }) {
      Scroll() {
        Column({ space: 20 }) {
          Row() {
            Column({ space: 6 }) {
              Text('GTA V HarmonyOS加载器').fontSize(28).fontWeight(FontWeight.Bold)
                .fontColor(LauncherColors.text)
              Text('导入预编译游戏压缩文件后，等待处理完毕即可开始游戏。').fontSize(14)
                .fontColor(LauncherColors.secondaryText)
            }.alignItems(HorizontalAlign.Start).layoutWeight(1)
            Text('v1.0.1').fontSize(13).fontColor(LauncherColors.secondaryText)
          }.width('100%')
          LauncherSectionCard({
            title: '资源与启动',
            content: () => { this.resourceAndStartupContent(); }
          })
          LauncherSectionCard({
            title: '文件导入',
            content: () => { this.importFilesContent(); }
          })
        }.width('100%').constraintSize({ maxWidth: 920 }).padding(24).alignItems(HorizontalAlign.Start)
      }.width('100%').height('100%').backgroundColor(LauncherColors.background)
    }
  }

  @Builder
  private savePicker() {
    SavePickerDialog({
      saves: this.saves,
      onSelect: (id: string) => { this.beginGame('load', id); },
      onClose: () => { this.closeSavePicker(); }
    })
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
        .onAreaChange((_old: Area, area: Area) => {
          this.resetInput(); this.gameWidth = Number(area.width); this.gameHeight = Number(area.height); this.readSafeArea();
        })

      FiveStarTouchControls({
        visible: this.controlsVisible && !this.keyboardVisible && !this.diagnosticsVisible && !this.exitConfirmVisible,
        inputEnabled: this.appForeground && !this.exitConfirmVisible,
        stateJson: this.virtualPadStateJson, resetEpoch: this.inputResetEpoch,
        controlsSettingsEpoch: this.controlsSettingsEpoch, controllerConnected: this.controllerConnected,
        safeLeft: this.safeLeft, safeRight: this.safeRight, safeTop: this.safeTop, safeBottom: this.safeBottom,
        closePanelsEpoch: this.closePanelsEpoch, mouseLook: this.effectiveMouseLook(), gameAspect: this.gameAspect,
        onOpenKeyboard: () => { this.toolbarExpanded = true; this.toggleKeyboard(); },
        onOpenSettings: () => { this.resetInput(); this.toolbarExpanded = true; },
        onSettingsChanged: (json: string) => { AppStorage.setOrCreate<string>('gameFiveStarSettings', json); },
        onStatusChanged: (status: string) => {
          if (this.touchStatusText !== status) { this.touchStatusText = status; }
        },
        onPanelChanged: (open: boolean) => {
          if (open || this.childPanelOpen) { this.resetInput(); }
          if (open) { this.toolbarExpanded = false; }
          this.childPanelOpen = open;
          if (!open && !this.keyboardVisible && !this.diagnosticsVisible && !this.exitConfirmVisible) { this.focusGame(); }
        }
      }).width('100%').height('100%')

      if (this.keyboardVisible || this.diagnosticsVisible || this.exitConfirmVisible) {
        Column().width('100%').height('100%').hitTestBehavior(HitTestMode.Block)
          .onTouch((event: TouchEvent) => { event.stopPropagation(); })
          .onMouse((event: MouseEvent) => { event.stopPropagation(); })
      }

      Column({ space: 8 }) {
        Row() { Blank().hitTestBehavior(HitTestMode.None); this.gameToolbar() }.width('100%')
        if (this.diagnosticsVisible) {
          Row() { Blank().hitTestBehavior(HitTestMode.None); this.performancePanel() }.width('100%')
        }
        Blank().hitTestBehavior(HitTestMode.None)
        if (this.keyboardVisible) {
          GameKeyboard({
            resetEpoch: this.inputResetEpoch, inputEnabled: this.appForeground && !this.exitConfirmVisible,
            viewWidth: this.gameWidth,
            viewHeight: this.gameHeight - this.safeTop - this.safeBottom - (this.gameWidth < 700 ? 48 : 0),
            onClose: () => { this.toggleKeyboard(); }
          })
        }
      }.width('100%').height('100%')
        .padding({ left: this.safeLeft + 12, right: this.safeRight + 12, top: this.safeTop + 12, bottom: this.safeBottom + 12 })
        .hitTestBehavior(HitTestMode.Transparent)
      }.width('100%').height('100%').backgroundColor('#000000')
        .id('game-input').focusable(true).defaultFocus(true)
        .onFocus(() => {
          this.gameFocused = this.appForeground && !this.exitConfirmVisible;
          game.setInputFocus(this.gameFocused);
        })
        .onBlur(() => { this.gameFocused = false; this.resetInput(); })
        .onTouch((event: TouchEvent) => {
          if (event.type === TouchType.Down) { this.focusGame(); }
        })
        .onKeyEventDispatch((event: KeyEvent) => { return this.externalKey(event); })
        .onMouse((event: MouseEvent) => { this.externalMouse(event); })
        .onAxisEvent((event: AxisEvent) => { this.externalAxis(event); })
        .onHover((hovered: boolean) => {
          this.mousePositionKnown = false;
          this.showPointer(!hovered || !this.effectiveMouseLook());
          if (!hovered) { this.releaseMouseButtons(); }
        })
    }
  }
}
