#pragma once
#include <napi/native_api.h>
void RegisterGameSurface(napi_env env, napi_value exports);
void ExportGameMethods(napi_env env, napi_value exports);
void ExportVulkanBufferDiagnostics(napi_env env, napi_value exports);
