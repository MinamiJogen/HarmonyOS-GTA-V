#include "game_surface.h"
#include "muguet_vulkan_buffer_probe.h"
#include "graphics_accel_diagnostics.h"
#include "graphics_spatial_diagnostics.h"
#include "graphics_frame_diagnostics.h"
#if MG_HAS_GAME_RUNTIME
#include "muguet_game_runtime.h"
#endif
#include <napi/native_api.h>
#include <hilog/log.h>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <fstream>
#include <string>

namespace {
constexpr unsigned int kDomain = 0x4100;
constexpr const char *kTag = "VulkanBufferProbe";
struct ProbeWork {
    napi_async_work work = nullptr;
    napi_deferred deferred = nullptr;
    MuguetVulkanBufferProbeConfig config{};
    std::string kind = "buffer";
    std::string directory, result, error;
};
std::string ReadString(napi_env env, napi_value value) {
    size_t size = 0;
    if (napi_get_value_string_utf8(env, value, nullptr, 0, &size) != napi_ok) return {};
    std::string text(size + 1, '\0');
    if (napi_get_value_string_utf8(env, value, text.data(), text.size(), &size) != napi_ok) return {};
    text.resize(size); return text;
}
std::string Quote(const std::string &text) {
    std::string result = "\"";
    for (unsigned char c : text) {
        if (c == '"' || c == '\\') { result += '\\'; result += static_cast<char>(c); }
        else if (c == '\n') result += "\\n";
        else if (c == '\r') result += "\\r";
        else if (c < 32) result += '?';
        else result += static_cast<char>(c);
    }
    return result + '"';
}
bool Save(const std::string &path, const std::string &text) {
    std::ofstream file(path, std::ios::binary | std::ios::trunc);
    if (!file) return false;
    file << text << '\n'; file.close(); return file.good();
}
template<class T> bool OptionalInteger(napi_env env, napi_value object, const char *name, T &destination, std::string &error) {
    bool present = false;
    if (napi_has_named_property(env, object, name, &present) != napi_ok) { error = "Cannot inspect request property."; return false; }
    if (!present) return true;
    napi_value value = nullptr; double number = 0;
    napi_get_named_property(env, object, name, &value);
    const double maximum = sizeof(T) == 4 ? 4294967295.0 : 9007199254740991.0;
    if (napi_get_value_double(env, value, &number) != napi_ok || !std::isfinite(number) ||
        number < 0 || number > maximum || std::floor(number) != number) {
        error = std::string("Expected a nonnegative safe integer for ") + name; return false;
    }
    destination = static_cast<T>(number); return true;
}
bool OptionalBoolean(napi_env env, napi_value object, const char *name, uint32_t &destination, std::string &error) {
    bool present = false;
    if (napi_has_named_property(env, object, name, &present) != napi_ok) { error = "Cannot inspect request property."; return false; }
    if (!present) return true;
    napi_value value = nullptr; bool boolean = false;
    napi_get_named_property(env, object, name, &value);
    if (napi_get_value_bool(env, value, &boolean) == napi_ok) { destination = boolean ? 1 : 0; return true; }
    uint32_t integer = 0;
    if (OptionalInteger(env, object, name, integer, error) && integer <= 1) { destination = integer; return true; }
    error = std::string("Expected boolean or 0/1 for ") + name; return false;
}
bool ReadProbeKind(napi_env env, napi_value object, std::string &kind, std::string &error) {
    bool present = false;
    if (napi_has_named_property(env, object, "probe_kind", &present) != napi_ok) {
        error = "Cannot inspect probe_kind."; return false;
    }
    if (!present) return true;
    napi_value value = nullptr;
    napi_get_named_property(env, object, "probe_kind", &value);
    kind = ReadString(env, value);
    if (kind == "buffer" || kind == "wgpu_atlas" || kind == "depth_downsample" ||
        kind == "luminance" || kind == "readback_pool" || kind == "graphics_accel" ||
        kind == "graphics_spatial" || kind == "graphics_frame" || kind == "wgpu_spatial") return true;
    error = "Unknown diagnostic probe_kind."; return false;
}
void Execute(napi_env, void *data) {
    auto *job = static_cast<ProbeWork *>(data);
    const bool atlas = job->kind == "wgpu_atlas";
    const bool depth = job->kind == "depth_downsample";
    const bool luminance = job->kind == "luminance";
    const bool graphicsAccel = job->kind == "graphics_accel";
    const bool spatial = job->kind == "graphics_spatial";
    const bool frame = job->kind == "graphics_frame";
    const bool pool = job->kind == "readback_pool";
    const bool wgpuSpatial = job->kind == "wgpu_spatial";
    const std::string stem = wgpuSpatial ? "wgpu-spatial" : spatial ? "graphics-spatial" : frame ? "graphics-frame" :
        pool ? "readback-pool" : graphicsAccel ? "graphics-accel" : luminance ? "luminance" :
        depth ? "depth-downsample" : atlas ? "wgpu-atlas" : "vulkan-buffer";
    const std::string resultPath = job->directory + "/" + stem + "-probe-result.json";
    const std::string tracePath = job->directory + "/" + stem +
        (job->kind == "buffer" ? "-probe-last.json" : "-probe-trace.jsonl");
    const std::string probe = job->kind;
    std::ifstream marker(job->directory + "/vulkan-buffer-probe-request.json");
    if (!marker) {
        job->error = "Diagnostic request marker is missing; the Vulkan probe was not executed.";
    } else if (!Save(resultPath, "{\"status\":\"running\",\"probe\":" + Quote(probe) + "}")) {
        job->error = "Cannot write the diagnostic result file.";
    } else {
        OH_LOG_Print(LOG_APP, LOG_INFO, kDomain, kTag,
            "PROBE_BEGIN kind=%{public}s allocation=%{public}u upload=%{public}u pool_policy=%{public}u iterations=%{public}u output=%{public}s",
            job->kind.c_str(), job->config.allocation, job->config.upload, job->config.pool_policy, job->config.iterations, resultPath.c_str());
        const char *result = nullptr;
        if (wgpuSpatial) {
#if MG_HAS_GAME_RUNTIME
            const char *oldPoolTrace = std::getenv("MUGUET_VK_POOL_TRACE");
            const bool hadPoolTrace = oldPoolTrace != nullptr;
            const std::string savedPoolTrace = oldPoolTrace ? oldPoolTrace : "";
            const std::string poolTrace = job->directory + "/wgpu-spatial-pool-trace.txt";
            setenv("MUGUET_VK_POOL_TRACE", poolTrace.c_str(), 1);
            result = mg_wgpu_spatial_probe_json(job->directory.c_str(), job->config.iterations);
            if (hadPoolTrace) setenv("MUGUET_VK_POOL_TRACE", savedPoolTrace.c_str(), 1);
            else unsetenv("MUGUET_VK_POOL_TRACE");
#else
            job->error = "The WGPU spatial runtime is not linked.";
#endif
        } else if (spatial) {
            result = muguet_graphics_spatial_probe_json(job->directory.c_str());
        } else if (frame) {
            result = muguet_graphics_frame_probe_json(job->directory.c_str());
        } else if (pool) {
#if MG_HAS_GAME_RUNTIME
            result = mg_readback_pool_probe_json(job->directory.c_str(), job->config.iterations);
#else
            job->error = "The readback pool runtime is not linked.";
#endif
        } else if (graphicsAccel) {
            result = muguet_graphics_accel_probe_json(job->directory.c_str());
        } else if (luminance) {
#if MG_HAS_GAME_RUNTIME
            result = mg_luminance_probe_json(job->directory.c_str(), job->config.iterations);
#else
            job->error = "The luminance runtime is not linked.";
#endif
        } else if (depth) {
#if MG_HAS_GAME_RUNTIME
            result = mg_depth_downsample_probe_json(job->directory.c_str(), job->config.iterations);
#else
            job->error = "The depth downsample runtime is not linked.";
#endif
        } else if (atlas) {
#if MG_HAS_GAME_RUNTIME
            result = mg_wgpu_atlas_probe_json(job->directory.c_str(), job->config.iterations);
#else
            job->error = "The wgpu atlas runtime is not linked.";
#endif
        } else {
            result = muguet_vulkan_buffer_probe_json(&job->config, tracePath.c_str());
        }
        job->result = result ? result : "{\"status\":\"fail\",\"error\":\"Probe returned no JSON\"}";
        if (!Save(resultPath, job->result)) job->error = "Cannot save the completed diagnostic result.";
        OH_LOG_Print(LOG_APP, LOG_INFO, kDomain, kTag, "PROBE_FINISHED kind=%{public}s output=%{public}s trace=%{public}s", job->kind.c_str(), resultPath.c_str(), tracePath.c_str());
    }
    if (!job->error.empty()) {
        job->result = "{\"status\":\"fail\",\"error\":" + Quote(job->error) + "}";
        Save(resultPath, job->result);
    }
}
void Complete(napi_env env, napi_status status, void *data) {
    auto *job = static_cast<ProbeWork *>(data);
    napi_value value = nullptr;
    if (status != napi_ok || !job->error.empty()) {
        const std::string message = job->error.empty() ? "Vulkan diagnostic async work failed." : job->error;
        napi_create_string_utf8(env, message.data(), message.size(), &value);
        napi_reject_deferred(env, job->deferred, value);
    } else {
        napi_create_string_utf8(env, job->result.data(), job->result.size(), &value);
        napi_resolve_deferred(env, job->deferred, value);
    }
    napi_delete_async_work(env, job->work); delete job;
}
napi_value Run(napi_env env, napi_callback_info info) {
    auto *job = new ProbeWork;
    muguet_vulkan_buffer_probe_defaults(&job->config);
    size_t count = 2; napi_value args[2]{};
    napi_get_cb_info(env, info, &count, args, nullptr, nullptr);
    napi_value promise = nullptr; napi_create_promise(env, &job->deferred, &promise);
    if (count == 2) job->directory = ReadString(env, args[0]);
    napi_valuetype type = napi_undefined;
    if (count == 2) napi_typeof(env, args[1], &type);
    if (count != 2 || job->directory.empty() || type != napi_object) job->error = "Expected filesDirectory and a probe configuration object.";
    if (job->error.empty()) ReadProbeKind(env, args[1], job->kind, job->error);
    if (job->kind == "depth_downsample" || job->kind == "luminance" || job->kind == "readback_pool" || job->kind == "wgpu_spatial") job->config.iterations = 1;
    if (job->error.empty()) OptionalInteger(env, args[1], "iterations", job->config.iterations, job->error);
    if (job->error.empty() && (job->kind == "depth_downsample" || job->kind == "luminance") &&
        (job->config.iterations < 1 || job->config.iterations > 8)) {
        job->error = "The compute differential probe requires 1 to 8 fixture repetitions.";
    }
    if (job->error.empty() && job->kind == "wgpu_atlas" &&
        (job->config.iterations < 1 || job->config.iterations > 256)) {
        job->error = "The wgpu atlas probe requires 1 to 256 iterations.";
    }
    if (job->error.empty() && job->kind == "readback_pool" &&
        (job->config.iterations < 1 || job->config.iterations > 64)) {
        job->error = "The readback pool probe requires 1 to 64 fixture repetitions.";
    }
    if (job->error.empty() && job->kind == "wgpu_spatial" &&
        (job->config.iterations < 1 || job->config.iterations > 4)) {
        job->error = "The WGPU spatial probe requires 1 to 4 fixture repetitions.";
    }
    if (job->error.empty() && job->kind == "buffer") {
#define READ(field) if (job->error.empty()) OptionalInteger(env, args[1], #field, job->config.field, job->error)
        READ(allocation); READ(upload); READ(pool_policy); READ(physical_device);
        READ(commands_per_pool); READ(reset_every); READ(target_usage); READ(target_bytes);
        READ(destination_offset); READ(target_binding_offset); READ(staging_binding_offset);
#undef READ
        if (job->error.empty()) OptionalBoolean(env, args[1], "validation_negative_control", job->config.validation_negative_control, job->error);
    }
    if (job->error.empty()) {
        napi_value name = nullptr; napi_create_string_utf8(env, "VulkanBufferProbe", NAPI_AUTO_LENGTH, &name);
        if (napi_create_async_work(env, nullptr, name, Execute, Complete, job, &job->work) != napi_ok ||
            napi_queue_async_work(env, job->work) != napi_ok) job->error = "Cannot queue Vulkan diagnostic work.";
    }
    if (!job->error.empty()) {
        napi_value message = nullptr; napi_create_string_utf8(env, job->error.data(), job->error.size(), &message);
        napi_reject_deferred(env, job->deferred, message);
        if (job->work) napi_delete_async_work(env, job->work);
        delete job;
    }
    return promise;
}
}

void ExportVulkanBufferDiagnostics(napi_env env, napi_value exports) {
    napi_property_descriptor method = {"runVulkanBufferProbe", nullptr, Run, nullptr, nullptr, nullptr, napi_default, nullptr};
    napi_define_properties(env, exports, 1, &method);
}
