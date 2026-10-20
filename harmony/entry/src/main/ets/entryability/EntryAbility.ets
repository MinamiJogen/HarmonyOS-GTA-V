import { UIAbility, Want } from '@kit.AbilityKit';
import { window } from '@kit.ArkUI';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { systemDateTime } from '@kit.BasicServicesKit';
import { registerCrashEvidence } from './CrashEvidence';
import game from 'libharmony_probe.so';
import pointer from '@ohos.multimodalInput.pointer';
import { gamePerformanceCreate, gamePerformanceForeground, gamePerformanceBackground,
  gamePerformanceDestroy } from './GamePerformance';

export default class EntryAbility extends UIAbility {
  private startupToken: number = 0;
  private startupTimer: number = -1;
  private startupWindow: window.Window | undefined = undefined;
  private startupSizeListener: ((size: window.Size) => void) | undefined = undefined;

  private stopStartupObservation(): void {
    if (this.startupTimer >= 0) {
      clearTimeout(this.startupTimer);
      this.startupTimer = -1;
    }
    const observedWindow: window.Window | undefined = this.startupWindow;
    const listener: ((size: window.Size) => void) | undefined = this.startupSizeListener;
    if (observedWindow !== undefined && listener !== undefined) {
      try { observedWindow.off('windowSizeChange', listener); } catch (error) {
        hilog.warn(0x4100, 'HarmonyGame', 'STARTUP_SIZE_UNSUBSCRIBE_FAILED %{public}s', String(error));
      }
    }
    this.startupSizeListener = undefined;
  }

  private cancelStartupGate(): void {
    this.startupToken += 1;
    this.stopStartupObservation();
    this.startupWindow = undefined;
  }

  private startStartupGate(windowStage: window.WindowStage, mainWindow: window.Window, token: number): void {
    const nowMs: () => number = (): number =>
      systemDateTime.getUptime(systemDateTime.TimeType.STARTUP, false);
    const startedAt: number = nowMs();
    let lastSizeEventAt: number = startedAt;
    let sizeGeneration: number = 0;
    let observedGeneration: number = -1;
    let lastWidth: number = 0;
    let lastHeight: number = 0;
    let stableSince: number = -1;
    let finalRecheck: boolean = false;
    let requestsSettled: boolean = false;
    let orientationResult: number = -1;
    let contentLoadStarted: boolean = false;
    let timeoutLogged: boolean = false;
    let propertyFailureLogged: boolean = false;
    const current: () => boolean = (): boolean => token === this.startupToken &&
      this.startupWindow === mainWindow && !contentLoadStarted;

    // Subscribe before requesting rotation so the first transition is observed.
    this.startupSizeListener = (_size: window.Size): void => {
      if (!current()) { return; }
      sizeGeneration += 1;
      lastSizeEventAt = nowMs();
      stableSince = -1;
      finalRecheck = false;
    };
    mainWindow.on('windowSizeChange', this.startupSizeListener);

    mainWindow.setWindowKeepScreenOn(true).then(() => {
      hilog.info(0x4100, 'HarmonyGame', 'KEEP_SCREEN_ON enabled');
    }).catch((error: Error) => {
      hilog.error(0x4100, 'HarmonyGame', 'KEEP_SCREEN_ON failed: %{public}s', String(error));
    });

    const checkWindow: () => void = (): void => {
      if (!current()) { return; }
      this.startupTimer = -1;
      const now: number = nowMs();
      try {
        const rect: window.Rect = mainWindow.getWindowProperties().windowRect;
        const width: number = rect.width;
        const height: number = rect.height;
        const generation: number = sizeGeneration;
        const sameRect: boolean = width === lastWidth && height === lastHeight &&
          generation === observedGeneration;
        if (requestsSettled && width > height && height > 0) {
          if (!sameRect || stableSince < 0) {
            stableSince = now;
            finalRecheck = false;
          } else if (now - stableSince >= 300 && now - lastSizeEventAt >= 300) {
            if (finalRecheck) {
              // This is a further 100ms timer read of the same actual rect and
              // event generation, not an orientation-request acknowledgement.
              contentLoadStarted = true;
              this.stopStartupObservation();
              hilog.info(0x4100, 'HarmonyGame',
                'STARTUP_WINDOW_READY width=%{public}d height=%{public}d generation=%{public}d stableMs=%{public}d orientationResult=%{public}d',
                width, height, generation, now - stableSince, orientationResult);
              try {
                windowStage.loadContent('pages/Index').then(() => {
                  if (token === this.startupToken) {
                    hilog.info(0x4100, 'HarmonyGame', 'STARTUP_CONTENT_LOADED');
                  }
                }).catch((error: Error) => {
                  hilog.error(0x4100, 'HarmonyGame', 'STARTUP_CONTENT_LOAD_FAILED %{public}s', String(error));
                });
              } catch (error) {
                hilog.error(0x4100, 'HarmonyGame', 'STARTUP_CONTENT_LOAD_FAILED %{public}s', String(error));
              }
              return;
            }
            finalRecheck = true;
          }
        } else {
          stableSince = -1;
          finalRecheck = false;
        }
        lastWidth = width;
        lastHeight = height;
        observedGeneration = generation;
      } catch (error) {
        stableSince = -1;
        finalRecheck = false;
        if (!propertyFailureLogged) {
          propertyFailureLogged = true;
          hilog.error(0x4100, 'HarmonyGame', 'STARTUP_RECT_READ_FAILED %{public}s', String(error));
        }
      }
      if (!timeoutLogged && now - startedAt >= 5000) {
        timeoutLogged = true;
        hilog.warn(0x4100, 'HarmonyGame',
          'STARTUP_WINDOW_WAITING width=%{public}d height=%{public}d generation=%{public}d requestsSettled=%{public}s orientationResult=%{public}d',
          lastWidth, lastHeight, sizeGeneration, String(requestsSettled), orientationResult);
      }
      if (current()) { this.startupTimer = setTimeout(checkWindow, 100); }
    };
    this.startupTimer = setTimeout(checkWindow, 100);

    const requestLayout: () => Promise<void> = async (): Promise<void> => {
      try {
        const result: window.OrientationResult =
          // Keep both landscape orientations available to the sensor, including
          // a 180-degree turn while the system rotation switch is locked.
          await mainWindow.setPreferredOrientationWithResult(window.Orientation.AUTO_ROTATION_LANDSCAPE);
        orientationResult = result.executionResult;
        hilog.info(0x4100, 'HarmonyGame', 'STARTUP_ORIENTATION_RESULT %{public}d', orientationResult);
      } catch (error) {
        hilog.error(0x4100, 'HarmonyGame', 'Landscape request failed: %{public}s', String(error));
      }
      if (!current()) { return; }
      try { await mainWindow.setWindowLayoutFullScreen(true); } catch (error) {
        hilog.error(0x4100, 'HarmonyGame', 'Fullscreen request failed: %{public}s', String(error));
      }
      if (!current()) { return; }
      try { await mainWindow.setWindowSystemBarEnable([]); } catch (error) {
        hilog.error(0x4100, 'HarmonyGame', 'System bar request failed: %{public}s', String(error));
      }
      if (!current()) { return; }
      requestsSettled = true;
      stableSince = -1;
      finalRecheck = false;
      lastSizeEventAt = nowMs();
    };
    requestLayout().catch((error: Error) => {
      hilog.error(0x4100, 'HarmonyGame', 'STARTUP_LAYOUT_SEQUENCE_FAILED %{public}s', String(error));
    });
  }

  private receiveGameSession(want: Want): void {
    const parameters: Record<string, Object> | undefined = want.parameters;
    if (parameters === undefined) { return; }
    if (parameters === null || typeof parameters !== 'object') {
      hilog.warn(0x4100, 'HarmonyGame', 'SESSION_REQUEST_IGNORED invalid parameters');
      return;
    }
    const action = parameters['muguetSessionAction'];
    const saveId = parameters['muguetSessionSaveId'];
    // Ordinary launcher/file-open Wants carry no session request and must not
    // replay a prior session or trigger a restart.
    if (action === undefined && saveId === undefined) { return; }
    if (typeof action !== 'string' || typeof saveId !== 'string') {
      hilog.warn(0x4100, 'HarmonyGame', 'SESSION_REQUEST_IGNORED invalid action/save id types');
      return;
    }
    if (action === 'load') {
      const prefix: string = 'userdata/Documents/Rockstar Games/GTA V/Default/SGTA5';
      const slot: string = saveId.slice(prefix.length);
      if (!saveId.startsWith(prefix) || slot.length !== 4 || !/^[0-9]{4}$/.test(slot) || Number(slot) > 15) {
        hilog.warn(0x4100, 'HarmonyGame', 'SESSION_REQUEST_IGNORED noncanonical story save id');
        return;
      }
    } else if (action !== 'new' || saveId !== '') {
      hilog.warn(0x4100, 'HarmonyGame', 'SESSION_REQUEST_IGNORED unsupported action or nonempty new save id');
      return;
    }
    // Index watches the action, so publish its fully validated save id first.
    // Native code independently rechecks the actual local save list on launch.
    AppStorage.setOrCreate<string>('gameSessionRequestedSaveId', saveId);
    AppStorage.setOrCreate<string>('gameSessionRequestedAction', action);
    hilog.info(0x4100, 'HarmonyGame', 'SESSION_REQUEST_ACCEPTED action=%{public}s', action);
  }

  onCreate(want: Want): void {
    AppStorage.setOrCreate<boolean>('gameAppForeground', false);
    AppStorage.setOrCreate<string>('gameSessionRequestedAction', '');
    AppStorage.setOrCreate<string>('gameSessionRequestedSaveId', '');
    this.receiveGameSession(want);
    registerCrashEvidence(this.context);
    gamePerformanceCreate(this.context);
  }

  onNewWant(want: Want): void {
    this.receiveGameSession(want);
  }

  onForeground(): void {
    AppStorage.setOrCreate<boolean>('gameAppForeground', true);
    gamePerformanceForeground();
    game.setInputFocus(true);
  }

  onBackground(): void {
    AppStorage.setOrCreate<boolean>('gameAppForeground', false);
    gamePerformanceBackground();
    game.setInputFocus(false);
    try { pointer.setPointerVisibleSync(true); } catch (error) {
      hilog.warn(0x4100, 'GameInput', 'BACKGROUND_POINTER_RESET %{public}s', String(error));
    }
  }

  onDestroy(): void {
    this.cancelStartupGate();
    gamePerformanceDestroy();
  }

  onWindowStageDestroy(): void {
    this.cancelStartupGate();
  }

  onWindowStageCreate(windowStage: window.WindowStage): void {
    this.cancelStartupGate();
    const token: number = this.startupToken;
    try {
      const mainWindow: window.Window = windowStage.getMainWindowSync();
      this.startupWindow = mainWindow;
      this.startStartupGate(windowStage, mainWindow, token);
    } catch (error) {
      this.cancelStartupGate();
      hilog.error(0x4100, 'HarmonyGame', 'STARTUP_WINDOW_GATE_FAILED %{public}s', String(error));
    }
  }
}
