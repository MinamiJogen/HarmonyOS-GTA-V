export const configure: (filesDirectory: string, fgEnabled: boolean, seconds: number, uiPrediction?: boolean, geometry?: boolean, sourceFps?: number, targetFps?: number, fusionAwareMetadata?: string, swapchainSampled?: boolean, finalDepthState?: boolean, rifeStream?: boolean) => boolean;
export const status: () => string;
export const stop: () => void;
export const inspectPublicInterfaces: (filesDirectory: string) => string;
export const inspectModelMetadata: (filesDirectory: string) => Promise<string>;
export const inspectRife: (filesDirectory: string) => Promise<string>;
export const inspectNpuFlow: (filesDirectory: string) => Promise<string>;
