import { UIAbility, Want } from '@kit.AbilityKit';
import { window } from '@kit.ArkUI';
import { hilog } from '@kit.PerformanceAnalysisKit';
import frameProbe from 'libgraphics_frame_surface.so';

// Explicit diagnostic Ability; the home/game entry does not navigate here.
export default class GraphicsFrameProbeAbility extends UIAbility {
  onCreate(want: Want): void {
    const rawEnabled = want.parameters ? want.parameters['fg_enabled'] : undefined;
    const rawSeconds = want.parameters ? want.parameters['fg_seconds'] : undefined;
    const rawUi = want.parameters ? want.parameters['fg_ui_prediction'] : undefined;
    const rawGeometry = want.parameters ? want.parameters['fg_geometry'] : undefined;
    const geometry: boolean = rawGeometry === true || rawGeometry === 'true' || rawGeometry === '1';
    const uiPrediction: boolean = rawUi === true || rawUi === 'true' || rawUi === '1';
    const enabled: boolean = rawEnabled !== false && rawEnabled !== 'false' && rawEnabled !== '0';
    const parsed: number = Number(rawSeconds === undefined ? 20 : rawSeconds);
    const seconds: number = Number.isFinite(parsed) ? Math.max(3, Math.min(60, Math.floor(parsed))) : 20;
    AppStorage.setOrCreate<boolean>('fgSurfaceEnabled', enabled);
    AppStorage.setOrCreate<number>('fgSurfaceSeconds', seconds);
    AppStorage.setOrCreate<boolean>('fgSurfaceUiPrediction', uiPrediction);
    AppStorage.setOrCreate<boolean>('fgSurfaceGeometry', geometry);
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
