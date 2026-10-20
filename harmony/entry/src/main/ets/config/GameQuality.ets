import { fileIo } from '@kit.CoreFileKit';
import { hilog } from '@kit.PerformanceAnalysisKit';

export interface GameQualitySelection {
  preset: string;
  superResolution: boolean;
  frameGeneration: boolean;
}
export interface GameQualityCapabilities {
  runtimeLinked: boolean;
  superResolutionAvailable: boolean;
  frameGenerationAvailable: boolean;
}

// Must run BEFORE defaults/quality write any config. Keep an unsupported request
// intact so the player can explicitly turn it off or install a capable package.
export function requireGameQualityCapabilities(selection: GameQualitySelection,
  capabilities: GameQualityCapabilities): void {
  if (!capabilities.runtimeLinked) { throw new Error('本安装包未包含游戏运行时。'); }
  if (selection.frameGeneration && !capabilities.frameGenerationAvailable) {
    throw new Error('本安装包未编译 AI 补帧。请明确关闭补帧后启动，或安装支持补帧的完整包；已保存选择保持不变。');
  }
  if (selection.superResolution && !capabilities.superResolutionAvailable) {
    throw new Error('本安装包未包含超分能力；已保存选择保持不变。');
  }
}

function readConfig(filesDir: string): Record<string, Object> {
  const path: string = `${filesDir}/config.json`;
  const text: string = fileIo.accessSync(path) ? fileIo.readTextSync(path) : '{}';
  const config: Record<string, Object> = JSON.parse(text) as Record<string, Object>;
  if (config === null || typeof config !== 'object' || Array.isArray(config)) {
    throw new Error('config.json 顶层必须为对象，原配置保持不变。');
  }
  return config;
}

function writeConfig(filesDir: string, config: Record<string, Object>): void {
  const path: string = `${filesDir}/config.json`;
  const temporary: string = `${path}.quality.tmp`;
  const text: string = `${JSON.stringify(config, null, 2)}\n`;
  const file: fileIo.File = fileIo.openSync(temporary,
    fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.CREATE | fileIo.OpenMode.TRUNC);
  try {
    fileIo.writeSync(file.fd, text);
    fileIo.fsyncSync(file.fd);
  } finally {
    fileIo.closeSync(file);
  }
  if (fileIo.readTextSync(temporary) !== text) { throw new Error('画质配置写入不完整，原配置保持不变。'); }
  fileIo.renameSync(temporary, path);
}

export function loadGameQuality(filesDir: string): GameQualitySelection {
  const config: Record<string, Object> = readConfig(filesDir);
  const preset: Object | undefined = config['graphics_preset'];
  return {
    preset: preset === 'high' || preset === 'low' ? preset as string : 'medium',
    superResolution: typeof config['spatial_upscale_enabled'] === 'boolean' ?
      config['spatial_upscale_enabled'] as boolean : true,
    frameGeneration: typeof config['frame_interpolation_enabled'] === 'boolean' ?
      config['frame_interpolation_enabled'] as boolean : true
  };
}

function setSelection(config: Record<string, Object>, selection: GameQualitySelection): void {
  if (selection.preset !== 'high' && selection.preset !== 'medium' && selection.preset !== 'low') {
    throw new Error('未知画质预设');
  }
  config['graphics_preset'] = selection.preset;
  config['spatial_upscale_enabled'] = selection.superResolution;
  config['frame_interpolation_enabled'] = selection.frameGeneration;
}

export function gameQualityArguments(preset: string): string[] {
  // These are the original game's options, not upscaler or resolution presets.
  // Specify the complete graphics baseline so switching back from high/low does
  // not inherit fields from an old settings.xml or a previous preset.
  const low: boolean = preset === 'low';
  const high: boolean = preset === 'high';
  return [
    `-textureQuality=${low ? 0 : (high ? 2 : 1)}`,
    // The shipped LQ shader set is incomplete (including required radar vars).
    // The user selected the lowest runnable shader level, with other low items at minimum.
    `-shaderQuality=${high ? 2 : 1}`,
    `-shadowQuality=${low ? -1 : (high ? 2 : 0)}`,
    `-shadowSoftness=${low ? 0 : 1}`,
    `-reflectionQuality=${high ? 1 : 0}`,
    '-reflectionMSAA=0', '-multiSample=0', '-multiSampleQuality=0', '-samplingMode=0', '-txaa=0',
    `-reflectionBlur=${low ? 0 : 1}`,
    `-fxaa=${low ? 0 : 1}`,
    `-anisotropicQualityLevel=${low ? 0 : (high ? 8 : 4)}`,
    `-SSAO=${high ? 1 : 0}`,
    `-particleQuality=${high ? 1 : 0}`,
    `-waterQuality=${high ? 1 : 0}`,
    `-grassQuality=${low ? 0 : (high ? 2 : 1)}`,
    `-postFX=${high ? 1 : 0}`,
    '-tessellation=0',
    `-lodScale=${low ? 0 : (high ? 0.35 : 0.1)}`,
    `-pedLodBias=${high ? 0.10 : 0}`,
    `-vehicleLodBias=${high ? 0.10 : 0}`,
    '-cityDensity=0',
    `-pedVariety=${low ? 0 : 1}`,
    `-vehicleVariety=${low ? 0 : 1}`,
    '-MaxLODScale=0', '-MotionBlurStrength=0',
    '-ultraShadows=0', '-particleShadows=0', '-shadowLongShadows=0',
    '-shadowDistance=1', '-shadowDisableScreenSizeCheck=0',
    '-shadowSplitZStart=0.93', '-shadowSplitZEnd=0.89', '-shadowAircraftWeight=0.99',
    '-fogVolumes=0', '-HDStreamingInFlight=0', '-noInGameDOF=0'
  ];
}

export function saveGameQuality(filesDir: string, selection: GameQualitySelection): void {
  const config: Record<string, Object> = readConfig(filesDir);
  setSelection(config, selection);
  writeConfig(filesDir, config);
}

export function applyGameQuality(filesDir: string, selection: GameQualitySelection): void {
  const original: Record<string, Object> = readConfig(filesDir);
  const config: Record<string, Object> = {};
  for (const key of Object.keys(original)) {
    if (key !== 'spatial_upscale_input_size' && key !== 'spatial_upscale_output_size') {
      config[key] = original[key];
    }
  }
  setSelection(config, selection);
  const engineArgs: string[] = gameQualityArguments(selection.preset);
  const managed: string[] = engineArgs.map((arg: string) => arg.split('=')[0].toLowerCase());
  const existing: Object | undefined = config['extra_args'];
  const extra: string[] = Array.isArray(existing) ? existing as string[] : [];
  const preserved: string[] = extra.filter((arg: string) =>
    typeof arg === 'string' && managed.indexOf(arg.split('=')[0].toLowerCase()) < 0);
  config['extra_args'] = preserved.concat(engineArgs);
  config['graphics_profile_version'] = 1;
  config['texture_quality'] = selection.preset === 'low' ? 0 : (selection.preset === 'high' ? 2 : 1);
  config['no_msaa'] = true;
  config['scale'] = 1.0;
  config['fps_limit'] = 24;
  config['spatial_upscale_mode'] = 'native_quality';
  config['spatial_upscale_match_display'] = true;
  config['spatial_upscale_native_short_edge'] = 800;
  config['spatial_upscale_output_short_edge'] = 1080;
  config['spatial_upscale_sharpness'] = 0.1;
  config['frame_interpolation_output_fps'] = 40;
  config['frame_interpolation_classifier_policy'] = 'observe_only';
  writeConfig(filesDir, config);
  hilog.info(0x4100, 'GameQuality', 'QUALITY_LAUNCH preset=%{public}s sr=%{public}s fg=%{public}s',
    selection.preset, String(selection.superResolution), String(selection.frameGeneration));
}
