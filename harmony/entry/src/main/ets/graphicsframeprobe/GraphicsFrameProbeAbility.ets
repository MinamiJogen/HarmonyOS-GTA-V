import { bundleManager, UIAbility, Want } from '@kit.AbilityKit';
import { window } from '@kit.ArkUI';
import { FaultLogger, hilog } from '@kit.PerformanceAnalysisKit';
import { fileIo } from '@kit.CoreFileKit';
import frameProbe from 'libgraphics_frame_surface.so';
import { registerCrashEvidence } from '../entryability/CrashEvidence';

// Explicit diagnostic Ability; the home/game entry does not navigate here.
export default class GraphicsFrameProbeAbility extends UIAbility {
  onCreate(want: Want): void {
    registerCrashEvidence(this.context);
    const rawInspect = want.parameters ? want.parameters['fg_inspect_only'] : undefined;
    const inspectOnly: boolean = rawInspect === true || rawInspect === 'true' || rawInspect === '1';
    AppStorage.setOrCreate<boolean>('fgSurfaceInspectOnly', inspectOnly);
    const rawRifeStream = want.parameters ? want.parameters['fg_rife_stream'] : undefined;
    const rifeStream: boolean = rawRifeStream === true || rawRifeStream === 'true' || rawRifeStream === '1';
    AppStorage.setOrCreate<boolean>('fgSurfaceRifeStream', rifeStream);
    if (inspectOnly) {
      const rawNpuFlow = want.parameters ? want.parameters['fg_npu_flow_benchmark'] : undefined;
      if (rawNpuFlow === true || rawNpuFlow === 'true' || rawNpuFlow === '1') {
        frameProbe.inspectNpuFlow(this.context.filesDir).catch((e: Error) => {
          hilog.error(0x4100, 'FgSurfaceProbe', 'NPU flow benchmark failed %{public}s', String(e));
        });
      }
      const rawRife = want.parameters ? want.parameters['fg_rife_benchmark'] : undefined;
      if (rawRife === true || rawRife === 'true' || rawRife === '1') {
        frameProbe.inspectRife(this.context.filesDir).catch((e: Error) => {
          hilog.error(0x4100, 'FgSurfaceProbe', 'RIFE benchmark failed %{public}s', String(e));
        });
      }
      const rawModelMetadata = want.parameters ? want.parameters['fg_model_metadata'] : undefined;
      if (rawModelMetadata === true || rawModelMetadata === 'true' || rawModelMetadata === '1') {
        // NNRT compilation runs on a native async worker, keeping the UI and
        // interface/crash collection responsive during this explicit audit.
        frameProbe.inspectModelMetadata(this.context.filesDir).catch((e: Error) => {
          hilog.error(0x4100, 'FgSurfaceProbe', 'model metadata audit failed %{public}s', String(e));
        });
      }
      // The current watcher receives future crashes. This public diagnostic
      // query also retrieves this bundle's older cached C++ crash records.
      FaultLogger.query(FaultLogger.FaultType.CPP_CRASH).then((logs: FaultLogger.FaultLogInfo[]) => {
        const file = fileIo.openSync(`${this.context.filesDir}/fg-archived-crashes.json`,
          fileIo.OpenMode.CREATE | fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.TRUNC);
        try { fileIo.writeSync(file.fd, JSON.stringify(logs)); } finally { fileIo.closeSync(file); }
      }).catch((e: Error) => {
        hilog.error(0x4100, 'FgSurfaceProbe', 'archived self crash query failed %{public}s', String(e));
      });
    }
    const rawEnabled = want.parameters ? want.parameters['fg_enabled'] : undefined;
    const rawSeconds = want.parameters ? want.parameters['fg_seconds'] : undefined;
    const rawUi = want.parameters ? want.parameters['fg_ui_prediction'] : undefined;
    const rawGeometry = want.parameters ? want.parameters['fg_geometry'] : undefined;
    const rawSourceFps = want.parameters ? want.parameters['fg_source_fps'] : undefined;
    const rawTargetFps = want.parameters ? want.parameters['fg_target_fps'] : undefined;
    const rawSwapchainSampled = want.parameters ? want.parameters['fg_swapchain_sampled'] : undefined;
    const rawFinalDepthState = want.parameters ? want.parameters['fg_final_depth_state'] : undefined;
    const geometry: boolean = rawGeometry === true || rawGeometry === 'true' || rawGeometry === '1';
    const uiPrediction: boolean = rawUi === true || rawUi === 'true' || rawUi === '1';
    const swapchainSampled: boolean = rawSwapchainSampled === true || rawSwapchainSampled === 'true' || rawSwapchainSampled === '1';
    const finalDepthState: boolean = rawFinalDepthState === true || rawFinalDepthState === 'true' || rawFinalDepthState === '1';
    const enabled: boolean = !rifeStream && rawEnabled !== false && rawEnabled !== 'false' && rawEnabled !== '0';
    const parsed: number = Number(rawSeconds === undefined ? 20 : rawSeconds);
    const seconds: number = Number.isFinite(parsed) ? Math.max(3, Math.min(60, Math.floor(parsed))) : 20;
    // Preserve invalid values for native validation instead of silently changing
    // the requested rate. Explicit 30/60 reproduces the earlier source cadence.
    const sourceFps: number = Number(rawSourceFps === undefined ? 15 : rawSourceFps);
    const targetFps: number = Number(rawTargetFps === undefined ? 30 : rawTargetFps);
    let fusionAwareMetadata: string = 'unavailable';
    try {
      const flags: number = bundleManager.BundleFlag.GET_BUNDLE_INFO_WITH_HAP_MODULE |
        bundleManager.BundleFlag.GET_BUNDLE_INFO_WITH_METADATA;
      const bundle = bundleManager.getBundleInfoForSelfSync(flags);
      const currentModule: string = this.context.currentHapModuleInfo.name;
      for (const hap of bundle.hapModulesInfo) {
        if (hap.name !== currentModule) { continue; }
        fusionAwareMetadata = 'absent';
        for (const item of hap.metadata) {
          if (item.name === 'GraphicsAccelerateKit_FusionAware') {
            fusionAwareMetadata = item.value;
            break;
          }
        }
        break;
      }
    } catch (e) {
      hilog.error(0x4100, 'FgSurfaceProbe', 'module metadata unavailable %{public}s', String(e));
    }
    AppStorage.setOrCreate<boolean>('fgSurfaceEnabled', enabled);
    AppStorage.setOrCreate<number>('fgSurfaceSeconds', seconds);
    AppStorage.setOrCreate<boolean>('fgSurfaceUiPrediction', uiPrediction);
    AppStorage.setOrCreate<boolean>('fgSurfaceGeometry', geometry);
    AppStorage.setOrCreate<boolean>('fgSurfaceSwapchainSampled', swapchainSampled);
    AppStorage.setOrCreate<boolean>('fgSurfaceFinalDepthState', finalDepthState);
    AppStorage.setOrCreate<number>('fgSurfaceSourceFps', sourceFps);
    AppStorage.setOrCreate<number>('fgSurfaceTargetFps', targetFps);
    AppStorage.setOrCreate<string>('fgSurfaceFusionAwareMetadata', fusionAwareMetadata);
  }

  onBackground(): void { frameProbe.stop(); }
  onDestroy(): void { frameProbe.stop(); }

  onWindowStageCreate(stage: window.WindowStage): void {
    const main: window.Window = stage.getMainWindowSync();
    main.setWindowKeepScreenOn(true).catch((e: Error) => {
      hilog.error(0x4100, 'FgSurfaceProbe', 'keep-screen-on %{public}s', String(e));
    });
    main.setPreferredOrientation(window.Orientation.LANDSCAPE).catch((e: Error) => {
      hilog.error(0x4100, 'FgSurfaceProbe', 'orientation %{public}s', String(e));
    });
    main.setWindowLayoutFullScreen(true).catch((e: Error) => {
      hilog.error(0x4100, 'FgSurfaceProbe', 'fullscreen %{public}s', String(e));
    });
    main.setWindowSystemBarEnable([]).catch((e: Error) => {
      hilog.error(0x4100, 'FgSurfaceProbe', 'system bars %{public}s', String(e));
    });
    stage.loadContent('pages/GraphicsFrameProbe');
  }
}
