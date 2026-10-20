import { fileIo } from '@kit.CoreFileKit';
import { TouchLayoutSettings } from './TouchSettings';

/** Independent host settings file; never modifies the guest's performance/quality/save configuration. */
export function readTouchPreferences(filesDir: string): TouchLayoutSettings {
  const path: string = `${filesDir}/touch-layout-v1.json`;
  if (!fileIo.accessSync(path)) { return { version: 1, families: {} }; }
  const settings: TouchLayoutSettings = JSON.parse(fileIo.readTextSync(path)) as TouchLayoutSettings;
  return settings && settings.version === 1 && settings.families ? settings : { version: 1, families: {} };
}
export function writeTouchPreferences(filesDir: string, settings: TouchLayoutSettings): void {
  const path: string = `${filesDir}/touch-layout-v1.json`;
  const temporary: string = `${path}.tmp`;
  const text: string = JSON.stringify(settings);
  const file: fileIo.File = fileIo.openSync(temporary, fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.CREATE | fileIo.OpenMode.TRUNC);
  try { fileIo.writeSync(file.fd, text); fileIo.fsyncSync(file.fd); } finally { fileIo.closeSync(file); }
  if (fileIo.readTextSync(temporary) !== text) { throw new Error('触控布局写入不完整'); }
  fileIo.renameSync(temporary, path);
}
