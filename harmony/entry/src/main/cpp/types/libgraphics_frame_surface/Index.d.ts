export const configure: (filesDirectory: string, fgEnabled: boolean, seconds: number, uiPrediction?: boolean, geometry?: boolean) => boolean;
export const status: () => string;
export const stop: () => void;
