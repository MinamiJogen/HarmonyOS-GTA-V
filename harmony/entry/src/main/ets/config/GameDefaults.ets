import { fileIo } from '@kit.CoreFileKit';
import { hilog } from '@kit.PerformanceAnalysisKit';

const PLAYABLE_DEFAULTS_VERSION: number = 1;
const PROFILE_VERSION_KEY: string = 'playable_defaults_version';

// Playable low-graphics defaults, excluding device-specific resolution.
// Existing player resolution is kept.
// This profile is applied once; subsequent explicit player changes are retained.
const PLAYABLE_PROFILE: string = `{
  "scale": 0.4,
  "mouse_sensitivity": 2.0,
  "fullscreen": true,
  "fps_limit": 60,
  "start_mode": 2,
  "mute": false,
  "multiplayer": false,
  "host": false,
  "server": "",
  "player_name": "",
  "extra_args": ["-shadowQuality=-1", "-reflectionQuality=0"],
  "cpu_profile": false,
  "gpu_diagnostics": false,
  "sync_submit": false,
  "gpu_profile": false,
  "thread_qos": true,
  "depth_reduce_opt": true,
  "depth_reduce_no_zero": true,
  "skip_empty_init_transition": false,
  "readback_pool": false,
  "luminance_no_zero": true,
  "spatial_upscale_mode": "native_quality",
  "spatial_upscale_match_display": true
}`;

function parseConfig(text: string): Record<string, Object> {
  let config: Record<string, Object>;
  try {
    config = JSON.parse(text) as Record<string, Object>;
  } catch (_) {
    throw new Error('config.json JSON 已损坏，原文件保持不变，请先修复配置。');
  }
  if (config === null || typeof config !== 'object' || Array.isArray(config)) {
    throw new Error('config.json 顶层必须是 JSON 对象，原文件保持不变。');
  }
  return config;
}

function writeTextAtomically(path: string, text: string): void {
  // The temporary file shares the destination directory and filesystem.
  const temporaryPath: string = `${path}.playable-defaults-v1.tmp`;
  const file: fileIo.File = fileIo.openSync(temporaryPath,
    fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.CREATE | fileIo.OpenMode.TRUNC);
  try {
    fileIo.writeSync(file.fd, text);
    fileIo.fsyncSync(file.fd);
  } finally {
    fileIo.closeSync(file);
  }
  // Catch short writes before replacing any existing configuration or backup.
  if (fileIo.readTextSync(temporaryPath) !== text) {
    throw new Error('配置临时文件写入不完整，原文件保持不变。');
  }
  fileIo.renameSync(temporaryPath, path);
}

export function applyGameDefaults(filesDir: string): void {
  const configPath: string = `${filesDir}/config.json`;
  const backupPath: string = `${filesDir}/config.before-playable-defaults-v1.json`;
  const existed: boolean = fileIo.accessSync(configPath);
  const originalText: string = existed ? fileIo.readTextSync(configPath) : '{}';
  const config: Record<string, Object> = parseConfig(originalText);
  const existingVersion: Object | undefined = config[PROFILE_VERSION_KEY];
  if (typeof existingVersion === 'number' && Number.isFinite(existingVersion) &&
    Number.isInteger(existingVersion) && existingVersion >= PLAYABLE_DEFAULTS_VERSION) {
    return;
  }

  const profile: Record<string, Object> = parseConfig(PLAYABLE_PROFILE);
  const keys: string[] = Object.keys(profile);
  for (const key of keys) {
    config[key] = profile[key];
  }
  if (config['spatial_upscale_sharpness'] === undefined) {
    config['spatial_upscale_sharpness'] = 0.2;
  }
  config[PROFILE_VERSION_KEY] = PLAYABLE_DEFAULTS_VERSION;
  const updatedText: string = `${JSON.stringify(config, null, 2)}\n`;

  // Retain the very first pre-migration configuration, including unknown fields.
  if (existed && !fileIo.accessSync(backupPath)) {
    writeTextAtomically(backupPath, originalText);
  }
  writeTextAtomically(configPath, updatedText);
  hilog.info(0x4100, 'GameDefaults', 'PLAYABLE_DEFAULTS_APPLIED version=%{public}d existing=%{public}s profile=MatePadProMax-20261018',
    PLAYABLE_DEFAULTS_VERSION, String(existed));
}
