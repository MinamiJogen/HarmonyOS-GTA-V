#include "fg_npu_flow.h"

#include <CANNKit/hiai_helper.h>
#include <CANNKit/hiai_options.h>
#include <neural_network_runtime/neural_network_core.h>

#include <algorithm>
#include <array>
#include <cerrno>
#include <chrono>
#include <cstdio>
#include <cstring>
#include <dlfcn.h>
#include <fcntl.h>
#include <memory>
#include <new>
#include <stdexcept>
#include <string>
#include <sys/stat.h>
#include <unistd.h>
#include <vector>

namespace {
constexpr size_t kInputBytes = MUGUET_FG_NPU_FLOW_INPUT_BYTES;
constexpr size_t kFlowElements = MUGUET_FG_NPU_FLOW_HALF_ELEMENTS;
constexpr size_t kFlowBytes = kFlowElements * sizeof(uint16_t);
constexpr size_t kLogitBytes = 2 * sizeof(float);
constexpr uint64_t kMaxModelBytesExclusive = 8 * 1024 * 1024;
constexpr char kModelPath[] = "/sys_prod/etc/graphics_game/ai_model/aifi2.om";
thread_local char threadError[512]{};

template<size_t N> void CopyText(char (&destination)[N], const char* text) noexcept
{
    std::snprintf(destination, N, "%s", text ? text : "");
}

struct Failure : std::runtime_error {
    int32_t code;
    Failure(int32_t value, const std::string& text) : std::runtime_error(text), code(value) {}
};

void RequireSuccess(OH_NN_ReturnCode code, int32_t failure, const char* stage)
{
    if (code != OH_NN_SUCCESS)
        throw Failure(failure, std::string(stage) + " failed, public code=" + std::to_string(int(code)));
}

struct Api {
    // Deliberately retained: backend threads/TLS callbacks can outlive handles.
    void* nnrt = nullptr;
    void* hiai = nullptr;
    std::string error;
#define FG_FLOW_NNRT_APIS(X) \
    X(OH_NNDevice_GetAllDevicesID) X(OH_NNDevice_GetName) X(OH_NNDevice_GetType) \
    X(OH_NNCompilation_ConstructWithOfflineModelBuffer) X(OH_NNCompilation_SetDevice) \
    X(OH_NNCompilation_Build) X(OH_NNCompilation_Destroy) X(OH_NNExecutor_Construct) \
    X(OH_NNExecutor_Destroy) X(OH_NNExecutor_GetInputCount) X(OH_NNExecutor_GetOutputCount) \
    X(OH_NNExecutor_CreateInputTensorDesc) X(OH_NNExecutor_CreateOutputTensorDesc) \
    X(OH_NNTensorDesc_GetName) X(OH_NNTensorDesc_GetDataType) X(OH_NNTensorDesc_GetFormat) \
    X(OH_NNTensorDesc_GetShape) X(OH_NNTensorDesc_GetByteSize) X(OH_NNTensorDesc_Destroy) \
    X(OH_NNTensor_Create) X(OH_NNTensor_Destroy) X(OH_NNTensor_GetDataBuffer) \
    X(OH_NNTensor_GetSize) X(OH_NNExecutor_RunSync)
#define FG_FLOW_HIAI_APIS(X) \
    X(HMS_HiAI_GetVersion) X(HMS_HiAICompatibility_CheckFromBuffer) \
    X(HMS_HiAIOptions_SetModelDeviceOrder) X(HMS_HiAIOptions_SetFallbackMode) \
    X(HMS_HiAIOptions_GetModelDeviceCount) X(HMS_HiAIOptions_GetModelDeviceOrder) \
    X(HMS_HiAIOptions_GetFallbackMode)
#define FG_FLOW_DECLARE(name) decltype(&name) name = nullptr;
    FG_FLOW_NNRT_APIS(FG_FLOW_DECLARE)
    FG_FLOW_HIAI_APIS(FG_FLOW_DECLARE)
#undef FG_FLOW_DECLARE
    Api()
    {
        dlerror();
        nnrt = dlopen("libneural_network_runtime.so", RTLD_NOW | RTLD_LOCAL);
        if (!nnrt) { const char* message = dlerror(); error = message ? message : "NNRT dlopen failed"; return; }
        dlerror();
        hiai = dlopen("libhiai_foundation.so", RTLD_NOW | RTLD_LOCAL);
        if (!hiai) { const char* message = dlerror(); error = message ? message : "CANN dlopen failed"; return; }
#define FG_FLOW_LOAD_NNRT(name) \
        name = reinterpret_cast<decltype(name)>(dlsym(nnrt, #name)); \
        if (!name) error += std::string(error.empty() ? "" : "; ") + "Missing " #name;
#define FG_FLOW_LOAD_HIAI(name) \
        name = reinterpret_cast<decltype(name)>(dlsym(hiai, #name)); \
        if (!name) error += std::string(error.empty() ? "" : "; ") + "Missing " #name;
        FG_FLOW_NNRT_APIS(FG_FLOW_LOAD_NNRT)
        FG_FLOW_HIAI_APIS(FG_FLOW_LOAD_HIAI)
#undef FG_FLOW_LOAD_NNRT
#undef FG_FLOW_LOAD_HIAI
#undef FG_FLOW_NNRT_APIS
#undef FG_FLOW_HIAI_APIS
    }
};

struct ModelBytes {
    std::vector<uint8_t> bytes;
    ~ModelBytes()
    {
        volatile uint8_t* memory = bytes.data();
        for (size_t i = 0; i < bytes.size(); ++i) memory[i] = 0;
    }
};

struct Fd {
    int value;
    ~Fd() { if (value >= 0) close(value); }
};

void ReadModel(ModelBytes& model)
{
    Fd file{open(kModelPath, O_RDONLY | O_CLOEXEC)};
    if (file.value < 0) throw Failure(MUGUET_FG_NPU_FLOW_MODEL_UNAVAILABLE,
        "Stock aifi2 open failed, errno=" + std::to_string(errno));
    struct stat metadata{};
    if (fstat(file.value, &metadata) != 0) throw Failure(MUGUET_FG_NPU_FLOW_MODEL_UNAVAILABLE,
        "Stock aifi2 fstat failed, errno=" + std::to_string(errno));
    if (!S_ISREG(metadata.st_mode) || metadata.st_size <= 0 || uint64_t(metadata.st_size) >= kMaxModelBytesExclusive)
        throw Failure(MUGUET_FG_NPU_FLOW_MODEL_UNAVAILABLE, "Stock aifi2 must be regular with 0<size<8MiB");
    model.bytes.resize(size_t(metadata.st_size));
    size_t offset = 0;
    while (offset < model.bytes.size()) {
        const ssize_t count = read(file.value, model.bytes.data() + offset, model.bytes.size() - offset);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) throw Failure(MUGUET_FG_NPU_FLOW_MODEL_UNAVAILABLE, "Stock aifi2 short/failed read");
        offset += size_t(count);
    }
    uint8_t extra = 0;
    ssize_t count;
    do { count = read(file.value, &extra, 1); } while (count < 0 && errno == EINTR);
    if (count != 0) throw Failure(MUGUET_FG_NPU_FLOW_MODEL_UNAVAILABLE, "Stock aifi2 size changed or EOF check failed");
    for (auto& byte : model.bytes) byte = uint8_t((byte >> 1) | (byte << 7));
}

struct Handles {
    Api& api;
    OH_NNCompilation* compilation = nullptr;
    OH_NNExecutor* executor = nullptr;
    std::array<NN_Tensor*, 2> inputs{{nullptr, nullptr}}, outputs{{nullptr, nullptr}};
    ~Handles()
    {
        for (auto& tensor : outputs) if (tensor) api.OH_NNTensor_Destroy(&tensor);
        for (auto& tensor : inputs) if (tensor) api.OH_NNTensor_Destroy(&tensor);
        if (executor) api.OH_NNExecutor_Destroy(&executor);
        if (compilation) api.OH_NNCompilation_Destroy(&compilation);
    }
};

struct Descriptor {
    Api& api;
    NN_TensorDesc* value;
    ~Descriptor() { if (value) api.OH_NNTensorDesc_Destroy(&value); }
};

size_t SelectDevice(Api& api)
{
    const size_t* ids = nullptr;
    uint32_t count = 0;
    RequireSuccess(api.OH_NNDevice_GetAllDevicesID(&ids, &count), MUGUET_FG_NPU_FLOW_BACKEND_UNAVAILABLE,
        "OH_NNDevice_GetAllDevicesID");
    if (!ids || !count || count > 32)
        throw Failure(MUGUET_FG_NPU_FLOW_BACKEND_UNAVAILABLE, "Invalid public device ID list");
    const std::vector<size_t> copied(ids, ids + count);
    for (size_t id : copied) {
        const char* name = nullptr;
        OH_NN_DeviceType type = OH_NN_OTHERS;
        const auto nameCode = api.OH_NNDevice_GetName(id, &name);
        const auto typeCode = api.OH_NNDevice_GetType(id, &type);
        if (nameCode == OH_NN_SUCCESS && name && std::strcmp(name, "HIAI_F") == 0 &&
            typeCode == OH_NN_SUCCESS && type == OH_NN_ACCELERATOR) return id;
    }
    throw Failure(MUGUET_FG_NPU_FLOW_BACKEND_UNAVAILABLE, "Public HIAI_F accelerator was not observed after CANN load");
}

bool RegionsOverlap(const void* a, size_t aBytes, const void* b, size_t bBytes) noexcept
{
    const auto first = reinterpret_cast<uintptr_t>(a), second = reinterpret_cast<uintptr_t>(b);
    return first <= second ? second - first < aBytes : first - second < bBytes;
}
} // namespace

struct MuguetFgNpuFlowContext {
    Api& api;
    ModelBytes model; // Declared before handles: decoded bytes outlive compilation.
    Handles handles;
    std::array<void*, 2> inputMemory{}, outputMemory{};
    MuguetFgNpuFlowInfo info{};
    mutable char error[512]{};
    explicit MuguetFgNpuFlowContext(Api& value) : api(value), handles{value} {}
};

namespace {
void* AllocateTensor(MuguetFgNpuFlowContext& context, size_t device, bool input, size_t index)
{
    Api& api = context.api;
    Descriptor descriptor{api, input ? api.OH_NNExecutor_CreateInputTensorDesc(context.handles.executor, index) :
                                      api.OH_NNExecutor_CreateOutputTensorDesc(context.handles.executor, index)};
    if (!descriptor.value) throw Failure(MUGUET_FG_NPU_FLOW_ALLOCATION_FAILED, "Public tensor descriptor construction failed");
    const char* name = nullptr;
    int32_t* shape = nullptr;
    size_t rank = 0, bytes = 0;
    OH_NN_DataType type = OH_NN_UNKNOWN;
    OH_NN_Format format = OH_NN_FORMAT_NONE;
    RequireSuccess(api.OH_NNTensorDesc_GetName(descriptor.value, &name), MUGUET_FG_NPU_FLOW_SCHEMA_MISMATCH, "GetName");
    RequireSuccess(api.OH_NNTensorDesc_GetShape(descriptor.value, &shape, &rank), MUGUET_FG_NPU_FLOW_SCHEMA_MISMATCH, "GetShape");
    RequireSuccess(api.OH_NNTensorDesc_GetDataType(descriptor.value, &type), MUGUET_FG_NPU_FLOW_SCHEMA_MISMATCH, "GetDataType");
    RequireSuccess(api.OH_NNTensorDesc_GetFormat(descriptor.value, &format), MUGUET_FG_NPU_FLOW_SCHEMA_MISMATCH, "GetFormat");
    RequireSuccess(api.OH_NNTensorDesc_GetByteSize(descriptor.value, &bytes), MUGUET_FG_NPU_FLOW_SCHEMA_MISMATCH, "GetByteSize");
    const std::array<int32_t, 4> expected = input ? std::array<int32_t, 4>{{1, 4, 208, 448}} :
        (index == 0 ? std::array<int32_t, 4>{{1, 6, 208, 448}} : std::array<int32_t, 4>{{1, 2, 1, 1}});
    const auto expectedType = input ? OH_NN_UINT8 : (index == 0 ? OH_NN_FLOAT16 : OH_NN_FLOAT32);
    const size_t expectedBytes = input ? kInputBytes : (index == 0 ? kFlowBytes : kLogitBytes);
    if (!shape || rank != expected.size() || !std::equal(expected.begin(), expected.end(), shape) ||
        type != expectedType || format != OH_NN_FORMAT_NCHW || bytes != expectedBytes)
        throw Failure(MUGUET_FG_NPU_FLOW_SCHEMA_MISMATCH,
            std::string(input ? "Input" : "Output") + std::to_string(index) + " does not match fixed aifi2 448x208 schema");
    if (input) {
        CopyText(context.info.input_names[index], name);
        context.info.input_data_type = int32_t(type);
    } else {
        CopyText(context.info.output_names[index], name);
        if (index == 0) context.info.flow_data_type = int32_t(type);
        else context.info.logit_data_type = int32_t(type);
    }
    context.info.tensor_format = int32_t(format);
    NN_Tensor*& tensor = input ? context.handles.inputs[index] : context.handles.outputs[index];
    tensor = api.OH_NNTensor_Create(device, descriptor.value);
    if (!tensor) throw Failure(MUGUET_FG_NPU_FLOW_ALLOCATION_FAILED, "Public tensor allocation failed");
    size_t mappedBytes = 0;
    RequireSuccess(api.OH_NNTensor_GetSize(tensor, &mappedBytes), MUGUET_FG_NPU_FLOW_ALLOCATION_FAILED, "Tensor GetSize");
    void* memory = api.OH_NNTensor_GetDataBuffer(tensor);
    if (!memory || mappedBytes != expectedBytes)
        throw Failure(MUGUET_FG_NPU_FLOW_ALLOCATION_FAILED, "Tensor mapping or exact mapped byte size check failed");
    return memory;
}

void Initialize(MuguetFgNpuFlowContext& context)
{
    Api& api = context.api;
    const size_t device = SelectDevice(api);
    ReadModel(context.model);
    const auto compatibility = api.HMS_HiAICompatibility_CheckFromBuffer(context.model.bytes.data(), context.model.bytes.size());
    if (compatibility != HIAI_COMPATIBILITY_COMPATIBLE)
        throw Failure(MUGUET_FG_NPU_FLOW_MODEL_INCOMPATIBLE,
            "Transformed aifi2 compatibility failed, public code=" + std::to_string(int(compatibility)));
    auto& handles = context.handles;
    handles.compilation = api.OH_NNCompilation_ConstructWithOfflineModelBuffer(context.model.bytes.data(), context.model.bytes.size());
    if (!handles.compilation) throw Failure(MUGUET_FG_NPU_FLOW_ALLOCATION_FAILED, "Public compilation construction failed");
    RequireSuccess(api.OH_NNCompilation_SetDevice(handles.compilation, device), MUGUET_FG_NPU_FLOW_CONFIGURATION_FAILED, "SetDevice");
    HiAI_ExecuteDevice requested = HIAI_EXECUTE_DEVICE_NPU;
    RequireSuccess(api.HMS_HiAIOptions_SetModelDeviceOrder(handles.compilation, &requested, 1),
        MUGUET_FG_NPU_FLOW_CONFIGURATION_FAILED, "SetModelDeviceOrder(NPU)");
    RequireSuccess(api.HMS_HiAIOptions_SetFallbackMode(handles.compilation, HIAI_FALLBACK_DISABLED),
        MUGUET_FG_NPU_FLOW_CONFIGURATION_FAILED, "SetFallbackMode(DISABLED)");
    const size_t count = api.HMS_HiAIOptions_GetModelDeviceCount(handles.compilation);
    const auto* order = api.HMS_HiAIOptions_GetModelDeviceOrder(handles.compilation);
    if (count != 1 || !order || order[0] != HIAI_EXECUTE_DEVICE_NPU ||
        api.HMS_HiAIOptions_GetFallbackMode(handles.compilation) != HIAI_FALLBACK_DISABLED)
        throw Failure(MUGUET_FG_NPU_FLOW_CONFIGURATION_FAILED, "NPU-only/fallback-disabled options did not read back");
    RequireSuccess(api.OH_NNCompilation_Build(handles.compilation), MUGUET_FG_NPU_FLOW_CONFIGURATION_FAILED, "Build");
    handles.executor = api.OH_NNExecutor_Construct(handles.compilation);
    if (!handles.executor) throw Failure(MUGUET_FG_NPU_FLOW_ALLOCATION_FAILED, "Public executor construction failed");
    size_t inputs = 0, outputs = 0;
    RequireSuccess(api.OH_NNExecutor_GetInputCount(handles.executor, &inputs), MUGUET_FG_NPU_FLOW_SCHEMA_MISMATCH, "GetInputCount");
    RequireSuccess(api.OH_NNExecutor_GetOutputCount(handles.executor, &outputs), MUGUET_FG_NPU_FLOW_SCHEMA_MISMATCH, "GetOutputCount");
    if (inputs != 2 || outputs != 2)
        throw Failure(MUGUET_FG_NPU_FLOW_SCHEMA_MISMATCH, "aifi2 requires exactly two inputs and two outputs");
    for (size_t i = 0; i < 2; ++i) context.inputMemory[i] = AllocateTensor(context, device, true, i);
    for (size_t i = 0; i < 2; ++i) context.outputMemory[i] = AllocateTensor(context, device, false, i);
    auto& info = context.info;
    info.struct_size = sizeof(info);
    info.abi_version = MUGUET_FG_NPU_FLOW_ABI_VERSION;
    info.width = MUGUET_FG_NPU_FLOW_WIDTH;
    info.height = MUGUET_FG_NPU_FLOW_HEIGHT;
    info.input_bytes_per_frame = kInputBytes;
    info.flow_half_elements = kFlowElements;
    info.logit_elements = 2;
    info.npu_only_fallback_disabled = 1;
    info.backend_id = device;
    info.last_run_sync_code = OH_NN_SUCCESS;
    CopyText(info.backend_name, "HIAI_F");
    CopyText(info.cann_version, api.HMS_HiAI_GetVersion());
}
} // namespace

extern "C" int32_t muguet_fg_npu_flow_create(MuguetFgNpuFlowContext** out_context)
{
    threadError[0] = '\0';
    if (!out_context) { CopyText(threadError, "out_context is NULL"); return MUGUET_FG_NPU_FLOW_INVALID_ARGUMENT; }
    *out_context = nullptr;
    try {
        static Api api;
        if (!api.error.empty()) throw Failure(MUGUET_FG_NPU_FLOW_RUNTIME_UNAVAILABLE, api.error);
        auto context = std::make_unique<MuguetFgNpuFlowContext>(api);
        Initialize(*context);
        *out_context = context.release();
        return MUGUET_FG_NPU_FLOW_OK;
    } catch (const Failure& failure) {
        CopyText(threadError, failure.what()); return failure.code;
    } catch (const std::bad_alloc&) {
        CopyText(threadError, "Allocation failed"); return MUGUET_FG_NPU_FLOW_ALLOCATION_FAILED;
    } catch (const std::exception& exception) {
        CopyText(threadError, exception.what()); return MUGUET_FG_NPU_FLOW_INTERNAL_ERROR;
    } catch (...) {
        CopyText(threadError, "Unknown native exception"); return MUGUET_FG_NPU_FLOW_INTERNAL_ERROR;
    }
}

extern "C" int32_t muguet_fg_npu_flow_process_rgba8(MuguetFgNpuFlowContext* context,
    const uint8_t* input0, size_t input0_bytes, const uint8_t* input1, size_t input1_bytes,
    uint16_t* flow_half, size_t flow_half_capacity, float* logits, size_t logit_capacity)
{
    if (!context) { CopyText(threadError, "context is NULL"); return MUGUET_FG_NPU_FLOW_INVALID_ARGUMENT; }
    context->error[0] = '\0';
    if (!input0 || !input1 || input0_bytes != kInputBytes || input1_bytes != kInputBytes || !flow_half ||
        flow_half_capacity < kFlowElements || !logits || logit_capacity < 2 ||
        RegionsOverlap(flow_half, kFlowBytes, logits, kLogitBytes)) {
        CopyText(context->error, "Requires two exact packed RGBA8 448x208 inputs and nonoverlapping output capacities >=559104 half elements and >=2 floats");
        return MUGUET_FG_NPU_FLOW_INVALID_ARGUMENT;
    }
    try {
        std::memcpy(context->inputMemory[0], input0, kInputBytes);
        std::memcpy(context->inputMemory[1], input1, kInputBytes);
        const auto begin = std::chrono::steady_clock::now();
        const auto code = context->api.OH_NNExecutor_RunSync(context->handles.executor,
            context->handles.inputs.data(), 2, context->handles.outputs.data(), 2);
        const auto end = std::chrono::steady_clock::now();
        context->info.last_run_sync_ns = uint64_t(std::chrono::duration_cast<std::chrono::nanoseconds>(end - begin).count());
        context->info.last_run_sync_code = int32_t(code);
        RequireSuccess(code, MUGUET_FG_NPU_FLOW_RUN_FAILED, "RunSync");
        std::memcpy(flow_half, context->outputMemory[0], kFlowBytes);
        std::memcpy(logits, context->outputMemory[1], kLogitBytes);
        ++context->info.successful_runs;
        return MUGUET_FG_NPU_FLOW_OK;
    } catch (const Failure& failure) {
        CopyText(context->error, failure.what()); return failure.code;
    } catch (const std::exception& exception) {
        CopyText(context->error, exception.what()); return MUGUET_FG_NPU_FLOW_INTERNAL_ERROR;
    } catch (...) {
        CopyText(context->error, "Unknown native exception"); return MUGUET_FG_NPU_FLOW_INTERNAL_ERROR;
    }
}

extern "C" int32_t muguet_fg_npu_flow_get_info(const MuguetFgNpuFlowContext* context,
    MuguetFgNpuFlowInfo* info, size_t info_bytes)
{
    if (!context || !info || info_bytes != sizeof(MuguetFgNpuFlowInfo)) {
        if (context) CopyText(context->error, "get_info requires non-NULL info and exact info struct byte size");
        else CopyText(threadError, "get_info requires a live context");
        return MUGUET_FG_NPU_FLOW_INVALID_ARGUMENT;
    }
    std::memcpy(info, &context->info, sizeof(*info));
    return MUGUET_FG_NPU_FLOW_OK;
}

extern "C" const char* muguet_fg_npu_flow_last_error(const MuguetFgNpuFlowContext* context)
{
    return context ? context->error : threadError;
}

extern "C" void muguet_fg_npu_flow_destroy(MuguetFgNpuFlowContext* context)
{
    delete context;
}
