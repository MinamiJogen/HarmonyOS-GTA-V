#pragma once

#include <napi/native_api.h>

// listGameSaves(filesDir: string): Promise<string>
// importGameSaves(sourceFd: number, filesDir: string): Promise<string>
napi_value ExportGameSaveImport(napi_env env, napi_value exports);

// Use the SAME startup/import exclusion gate as resource import. No save import
// is allowed until the application installs this callback. begin=true reserves
// against an already-running/starting game; begin=false releases that reservation.
void SetGameSaveImportGate(bool (*gate)(bool begin));
