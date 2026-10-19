#pragma once

#include <napi/native_api.h>

// Register on the same exports object as the existing libharmony_probe API.
// importGameResources(fd: number, filesDir: string): Promise<string>
// checkGameResources(filesDir: string): Promise<string>
// getGameImportStatus(): string
// cancelGameImport(): void
napi_value ExportGameResourceImport(napi_env env, napi_value exports);

// The gate must atomically reserve import against game startup under the same
// mutex used by TryStartGame. true = reserve (false if a game has started),
// false = release. Import is disabled until the application supplies this gate.
void SetGameResourceImportGate(bool (*gate)(bool begin));
