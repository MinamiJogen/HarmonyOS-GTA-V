import { fileIo } from '@kit.CoreFileKit';

/**
 * HarmonyOS storage adapter for FiveStar touch preferences.
 * Copyright (c) 2026 GTA V iOS port contributors. Original source MIT; see
 * upstream/fivestar-input-8e2e4628/LICENSE and source.json.
 * Adapted at 8e2e4628. HarmonyOS adaptation distributed under GPL-3.0-or-later.
 * Kept separate from both the old touch layout and the guest's game/save files.
 */
export function readFiveStarPreferences(filesDir: string): string {
  const path: string = `${filesDir}/fivestar-controls-v1.json`;
  if (!fileIo.accessSync(path)) { return ''; }
  const text: string = fileIo.readTextSync(path);
  return text.length <= 262144 ? text : '';
}

export function writeFiveStarPreferences(filesDir: string, text: string): void {
  if (text.length > 262144) { throw new Error('触控设置超过保存上限'); }
  const path: string = `${filesDir}/fivestar-controls-v1.json`;
  const temporary: string = `${path}.tmp`;
  const file: fileIo.File = fileIo.openSync(temporary,
    fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.CREATE | fileIo.OpenMode.TRUNC);
  try {
    fileIo.writeSync(file.fd, text);
    fileIo.fsyncSync(file.fd);
  } finally {
    fileIo.closeSync(file);
  }
  if (fileIo.readTextSync(temporary) !== text) { throw new Error('触控设置写入不完整'); }
  fileIo.renameSync(temporary, path);
}
