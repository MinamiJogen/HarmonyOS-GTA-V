export interface EmbeddedCaseResult {
  name: string;
  available: boolean;
  attempted: boolean;
  passed: boolean;
  status: number;
  result: number;
  error: string;
  address: string;
  length: number;
}

export interface ProbeResult {
  cppReady: boolean;
  vulkanJson: string;
  vulkanReportPath: string;
  vulkanReportSaved: boolean;
  abi: string;
  rustLinked: boolean;
  rustVersion: string;
  rustResult: number;
  runtimeAvailable: boolean;
  aotAttempted: boolean;
  aotPassed: boolean;
  aotResult: number;
  aotStatus: number;
  error: string;
  embeddedAvailable: boolean;
  embeddedAttempted: boolean;
  embeddedPassed: boolean;
  embeddedStatus: number;
  embeddedResult: number;
  embeddedError: string;
  embeddedCases: EmbeddedCaseResult[];
}

export const runProbe: (aotPath: string, reportPath: string) => ProbeResult;

export interface GameStatus {
  frameGenerationAvailable?: boolean;
  uiAppForeground?: boolean;
  uiGameFocused?: boolean;
  uiMouseLook?: boolean;
  uiLastKeyCode?: number;
  uiLastAxisVertical?: number;
  uiLastAxisStep?: number;
  uiLastAxisNotches?: number;
  runtimeLinked: boolean;
  surfaceReady: boolean;
  width: number;
  height: number;
  startRequested: boolean;
  startDispatched: boolean;
  startResult: number;
  error: string;
  filesDirectory: string;
  runtimeJson: string;
}

export const configureGame: (filesDirectory: string, action?: string, saveId?: string, frameGenerationRequested?: boolean) => void;
export const startGame: () => void;
export const stopGame: () => void;
export const gameStatus: () => GameStatus;
export interface GameCapabilities {
  runtimeLinked: boolean;
  superResolutionAvailable: boolean;
  frameGenerationAvailable: boolean;
  scope: string;
}
/** Compile/link capabilities only; does not initialize NNRT, models or Vulkan. */
export const gameCapabilities: () => GameCapabilities;
/** Cached script-safe-point snapshot; native monotonic now/age accompany unchanged field timestamps. */
export const getTouchState: () => string;
/** Complete engine ioPad snapshots; not keyboard emulation. */
export const sendVirtualPad: (buttons: number, leftX: number, leftY: number, rightX: number, rightY: number,
  leftTrigger: number, rightTrigger: number, active: boolean) => boolean;
export const virtualPadState: () => string;
export const physicalControllerConnected: () => boolean;
export const releaseVirtualPad: () => void;
export const setVirtualRadio: (x: number, y: number, active: boolean) => void;
export const queueVirtualCameraSwipe: (x: number, y: number) => void;
export const sendMouseDelta: (dx: number, dy: number) => void;
export const setVirtualMouseMode: (active: boolean) => void;
export const requestVirtualPause: () => void;
export const nextVirtualAircraftWeapon: () => void;
export const setVirtualTargetingMode: (mode: number) => void;
export const sendControl: (controlName: string, value: number, group?: number) => boolean;
export const sendRawKey: (keyName: string, pressed: boolean, source?: string) => boolean;
export const releaseAllInputs: () => void;
export const releaseSemanticInputs: () => void;
export const checkGameResources: (filesDirectory: string) => Promise<string>;
export const importGameResources: (sourceFd: number, filesDirectory: string) => Promise<string>;
export const listGameSaves: (filesDirectory: string) => Promise<string>;
export const importGameSaves: (sourceFd: number, filesDirectory: string) => Promise<string>;
export const getGameImportStatus: () => string;
export const cancelGameImport: () => void;
export const sendKey: (keyName: string, pressed: boolean) => void;
export const sendKeyCode: (keyCode: number, pressed: boolean) => void;
export const sendMouseButton: (button: number, pressed: boolean, source?: string) => void;
export const sendKeyEvent: (keyCode: number, pressed: boolean, unicode: number, suppressText: boolean) => boolean;
export const setInputFocus: (focused: boolean) => void;
/** App visibility gates native presentation independently from input focus. */
export const setAppForeground: (foreground: boolean) => void;
export const sendPointer: (x: number, y: number, dx: number, dy: number, look: boolean) => void;
export const sendWheel: (notches: number) => void;

export interface VulkanBufferProbeRequest {
  probe_kind?: string;
  allocation?: number;
  upload?: number;
  pool_policy?: number;
  physical_device?: number;
  iterations?: number;
  commands_per_pool?: number;
  reset_every?: number;
  target_usage?: number;
  target_bytes?: number;
  destination_offset?: number;
  target_binding_offset?: number;
  staging_binding_offset?: number;
  validation_negative_control?: boolean;
}
export const runVulkanBufferProbe: (filesDirectory: string, request: VulkanBufferProbeRequest) => Promise<string>;
