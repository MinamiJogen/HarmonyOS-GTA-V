#include "fg_model_metadata_audit.h"

#include <neural_network_runtime/neural_network_core.h>
#include <CANNKit/hiai_helper.h>

#include <algorithm>
#include <cerrno>
#include <dlfcn.h>
#include <fcntl.h>
#include <mutex>
#include <sstream>
#include <sys/stat.h>
#include <unistd.h>
#include <utility>
#include <vector>

namespace muguet::fg_probe {
namespace {
constexpr size_t kMaxModels = 8;
constexpr uint32_t kMaxDevices = 32;
constexpr size_t kMaxTensors = 64;
constexpr size_t kMaxRank = 64;
constexpr size_t kMaxModelBytesExclusive = 8 * 1024 * 1024;

std::string Quote(const std::string& value)
{
    constexpr char hex[] = "0123456789abcdef";
    std::string out = "\"";
    for (unsigned char c : value) {
        if (c == '"' || c == '\\') { out += '\\'; out += char(c); }
        else if (c < 32) { out += "\\u00"; out += hex[c >> 4]; out += hex[c & 15]; }
        else { out += char(c); }
    }
    return out + '"';
}

std::string Array(const std::vector<std::string>& values)
{
    std::string out = "[";
    for (size_t i = 0; i < values.size(); ++i) {
        if (i) out += ',';
        out += values[i];
    }
    return out + ']';
}

const char* TypeName(OH_NN_DataType type)
{
    switch (type) {
        case OH_NN_UNKNOWN: return "UNKNOWN";
        case OH_NN_BOOL: return "BOOL";
        case OH_NN_INT8: return "INT8";
        case OH_NN_INT16: return "INT16";
        case OH_NN_INT32: return "INT32";
        case OH_NN_INT64: return "INT64";
        case OH_NN_UINT8: return "UINT8";
        case OH_NN_UINT16: return "UINT16";
        case OH_NN_UINT32: return "UINT32";
        case OH_NN_UINT64: return "UINT64";
        case OH_NN_FLOAT16: return "FLOAT16";
        case OH_NN_FLOAT32: return "FLOAT32";
        case OH_NN_FLOAT64: return "FLOAT64";
        default: return "UNRECOGNIZED";
    }
}

const char* FormatName(OH_NN_Format format)
{
    switch (format) {
        case OH_NN_FORMAT_NONE: return "NONE";
        case OH_NN_FORMAT_NCHW: return "NCHW";
        case OH_NN_FORMAT_NHWC: return "NHWC";
        case OH_NN_FORMAT_ND: return "ND";
        default: return "UNRECOGNIZED";
    }
}

struct Api {
    // Retain the public runtime for the process lifetime: NNRT may keep driver
    // threads alive after destroying the diagnostic executor/compilation.
    void* library = nullptr;
    std::vector<std::string> missingSymbols;
    std::string loaderError;
#define FG_NNRT_API_LIST(X) \
    X(OH_NNDevice_GetAllDevicesID) \
    X(OH_NNDevice_GetName) \
    X(OH_NNDevice_GetType) \
    X(OH_NNCompilation_ConstructWithOfflineModelFile) \
    X(OH_NNCompilation_ConstructWithOfflineModelBuffer) \
    X(OH_NNCompilation_SetDevice) \
    X(OH_NNCompilation_Build) \
    X(OH_NNCompilation_Destroy) \
    X(OH_NNExecutor_Construct) \
    X(OH_NNExecutor_Destroy) \
    X(OH_NNExecutor_GetInputCount) \
    X(OH_NNExecutor_GetOutputCount) \
    X(OH_NNExecutor_CreateInputTensorDesc) \
    X(OH_NNExecutor_CreateOutputTensorDesc) \
    X(OH_NNTensorDesc_GetName) \
    X(OH_NNTensorDesc_GetDataType) \
    X(OH_NNTensorDesc_GetShape) \
    X(OH_NNTensorDesc_GetFormat) \
    X(OH_NNTensorDesc_GetElementCount) \
    X(OH_NNTensorDesc_GetByteSize) \
    X(OH_NNTensorDesc_Destroy)
#define FG_DECLARE_API(name) decltype(&name) name = nullptr;
    FG_NNRT_API_LIST(FG_DECLARE_API)
#undef FG_DECLARE_API

    Api()
    {
        dlerror();
        library = dlopen("libneural_network_runtime.so", RTLD_NOW | RTLD_LOCAL);
        if (!library) {
            const char* error = dlerror();
            loaderError = error ? error : "dlopen returned null";
            return;
        }
#define FG_LOAD_API(name) \
        name = reinterpret_cast<decltype(name)>(dlsym(library, #name)); \
        if (!name) missingSymbols.push_back(Quote(#name));
        FG_NNRT_API_LIST(FG_LOAD_API)
#undef FG_LOAD_API
#undef FG_NNRT_API_LIST
    }
};

struct HiaiApi {
    void* library = nullptr;
    std::string loaderError;
    std::vector<std::string> missingSymbols;
    decltype(&HMS_HiAI_GetVersion) getVersion = nullptr;
    decltype(&HMS_HiAICompatibility_CheckFromFile) checkFile = nullptr;
    decltype(&HMS_HiAICompatibility_CheckFromBuffer) checkBuffer = nullptr;

    HiaiApi()
    {
        dlerror();
        library = dlopen("libhiai_foundation.so", RTLD_NOW | RTLD_LOCAL);
        if (!library) {
            const char* error = dlerror();
            loaderError = error ? error : "dlopen returned null";
            return;
        }
#define FG_LOAD_HIAI(member, name) \
        member = reinterpret_cast<decltype(member)>(dlsym(library, #name)); \
        if (!member) missingSymbols.push_back(Quote(#name));
        FG_LOAD_HIAI(getVersion, HMS_HiAI_GetVersion)
        FG_LOAD_HIAI(checkFile, HMS_HiAICompatibility_CheckFromFile)
        FG_LOAD_HIAI(checkBuffer, HMS_HiAICompatibility_CheckFromBuffer)
#undef FG_LOAD_HIAI
    }
    // Keep the backend mapped for the diagnostic process lifetime as for NNRT.
};

struct Device {
    size_t id;
    std::string name;
    OH_NN_ReturnCode nameCode;
    OH_NN_DeviceType type;
    OH_NN_ReturnCode typeCode;
    bool selected() const { return typeCode == OH_NN_SUCCESS && type == OH_NN_ACCELERATOR; }
    std::string json() const
    {
        return "{\"id\":" + std::to_string(id) + ",\"getNameCode\":" + std::to_string(int(nameCode)) +
            ",\"name\":" + (nameCode == OH_NN_SUCCESS ? Quote(name) : "null") +
            ",\"getTypeCode\":" + std::to_string(int(typeCode)) + ",\"type\":" +
            (typeCode == OH_NN_SUCCESS ? std::to_string(int(type)) : "null") +
            ",\"selected\":" + (selected() ? "true" : "false") + '}';
    }
};

struct DeviceSnapshot {
    OH_NN_ReturnCode code = OH_NN_FAILED;
    uint32_t reportedCount = 0;
    bool valid = false;
    std::vector<Device> devices;
    std::string json() const
    {
        std::vector<std::string> items;
        for (const auto& device : devices) items.push_back(device.json());
        return "{\"enumerateDevicesCode\":" + std::to_string(int(code)) +
            ",\"reportedDeviceCount\":" + (code == OH_NN_SUCCESS ? std::to_string(reportedCount) : "null") +
            ",\"valid\":" + (valid ? "true" : "false") + ",\"devices\":" + Array(items) + '}';
    }
};

DeviceSnapshot EnumerateDevices(Api& api)
{
    DeviceSnapshot snapshot;
    const size_t* ids = nullptr;
    snapshot.code = api.OH_NNDevice_GetAllDevicesID(&ids, &snapshot.reportedCount);
    snapshot.valid = snapshot.code == OH_NN_SUCCESS && snapshot.reportedCount <= kMaxDevices &&
        (!snapshot.reportedCount || ids);
    if (!snapshot.valid) return snapshot;
    std::vector<size_t> copiedIds;
    if (snapshot.reportedCount) copiedIds.assign(ids, ids + snapshot.reportedCount);
    for (size_t id : copiedIds) {
        const char* name = nullptr;
        OH_NN_DeviceType type = OH_NN_OTHERS;
        const auto nameCode = api.OH_NNDevice_GetName(id, &name);
        const auto typeCode = api.OH_NNDevice_GetType(id, &type);
        snapshot.devices.push_back({id, nameCode == OH_NN_SUCCESS && name ? name : "", nameCode, type, typeCode});
    }
    return snapshot;
}

struct TransformedModel {
    std::vector<uint8_t> bytes;
    std::string readStatus = "not_read";
    int readError = 0;
    int64_t fileSize = -1;
    bool compatibilityCalled = false;
    HiAI_Compatibility compatibility = HIAI_COMPATIBILITY_INCOMPATIBLE;
    TransformedModel() = default;
    TransformedModel(const TransformedModel&) = delete;
    TransformedModel& operator=(const TransformedModel&) = delete;
    TransformedModel(TransformedModel&&) = default;
    ~TransformedModel()
    {
        // Prevent an optimizer from removing erasure of transient model bytes.
        volatile uint8_t* data = bytes.data();
        for (size_t i = 0; i < bytes.size(); ++i) data[i] = 0;
    }
};

struct ReadFileHandle {
    int descriptor;
    ~ReadFileHandle() { if (descriptor >= 0) close(descriptor); }
};

void ReadAndTransform(const std::string& path, TransformedModel& model)
{
    errno = 0;
    ReadFileHandle file{open(path.c_str(), O_RDONLY | O_CLOEXEC)};
    if (file.descriptor < 0) { model.readStatus = "open_failed"; model.readError = errno; return; }
    struct stat metadata{};
    if (fstat(file.descriptor, &metadata) != 0) {
        model.readStatus = "fstat_failed"; model.readError = errno; return;
    }
    model.fileSize = metadata.st_size;
    if (!S_ISREG(metadata.st_mode) || metadata.st_size <= 0 ||
        uint64_t(metadata.st_size) >= kMaxModelBytesExclusive) {
        model.readStatus = "requires_regular_nonempty_file_strictly_below_8MiB";
        return;
    }
    model.bytes.resize(size_t(metadata.st_size));
    size_t offset = 0;
    while (offset < model.bytes.size()) {
        const ssize_t count = read(file.descriptor, model.bytes.data() + offset, model.bytes.size() - offset);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) {
            model.readStatus = count == 0 ? "short_read" : "read_failed";
            model.readError = count < 0 ? errno : 0;
            return;
        }
        offset += size_t(count);
    }
    uint8_t extra = 0;
    ssize_t extraCount;
    do { extraCount = read(file.descriptor, &extra, 1); } while (extraCount < 0 && errno == EINTR);
    if (extraCount != 0) {
        model.readStatus = extraCount < 0 ? "end_check_failed" : "file_grew_during_read";
        model.readError = extraCount < 0 ? errno : 0;
        return;
    }
    for (auto& byte : model.bytes) byte = uint8_t((byte >> 1) | (byte << 7));
    model.readStatus = "whole_file_read_and_ror1_applied_in_memory";
}

struct ModelHandles {
    Api& api;
    OH_NNCompilation* compilation = nullptr;
    OH_NNExecutor* executor = nullptr;
    explicit ModelHandles(Api& functions) : api(functions) {}
    ~ModelHandles()
    {
        if (executor) api.OH_NNExecutor_Destroy(&executor);
        if (compilation) api.OH_NNCompilation_Destroy(&compilation);
    }
};

struct DescriptorHandle {
    Api& api;
    NN_TensorDesc* descriptor;
    ~DescriptorHandle() { if (descriptor) api.OH_NNTensorDesc_Destroy(&descriptor); }
};

std::string TensorMetadata(Api& api, const OH_NNExecutor* executor, bool input,
                           size_t index, bool& complete)
{
    DescriptorHandle handle{api, input ? api.OH_NNExecutor_CreateInputTensorDesc(executor, index)
                                      : api.OH_NNExecutor_CreateOutputTensorDesc(executor, index)};
    std::ostringstream out;
    out << "{\"index\":" << index << ",\"descriptorReturnedNonNull\":"
        << (handle.descriptor ? "true" : "false");
    if (!handle.descriptor) { complete = false; return out.str() + '}'; }

    const char* name = nullptr;
    int32_t* shape = nullptr;
    size_t rank = 0, elements = 0, bytes = 0;
    OH_NN_DataType type = OH_NN_UNKNOWN;
    OH_NN_Format format = OH_NN_FORMAT_NONE;
    const auto nameCode = api.OH_NNTensorDesc_GetName(handle.descriptor, &name);
    const auto typeCode = api.OH_NNTensorDesc_GetDataType(handle.descriptor, &type);
    const auto shapeCode = api.OH_NNTensorDesc_GetShape(handle.descriptor, &shape, &rank);
    const auto formatCode = api.OH_NNTensorDesc_GetFormat(handle.descriptor, &format);
    const auto elementCode = api.OH_NNTensorDesc_GetElementCount(handle.descriptor, &elements);
    const auto bytesCode = api.OH_NNTensorDesc_GetByteSize(handle.descriptor, &bytes);
    const bool validShape = shapeCode == OH_NN_SUCCESS && rank <= kMaxRank && (!rank || shape);
    const bool valid = validShape && nameCode == OH_NN_SUCCESS && typeCode == OH_NN_SUCCESS &&
        formatCode == OH_NN_SUCCESS && elementCode == OH_NN_SUCCESS && bytesCode == OH_NN_SUCCESS;
    complete = complete && valid;
    out << ",\"getNameCode\":" << int(nameCode) << ",\"name\":"
        << (nameCode == OH_NN_SUCCESS && name ? Quote(name) : "null")
        << ",\"getDataTypeCode\":" << int(typeCode) << ",\"dataType\":"
        << (typeCode == OH_NN_SUCCESS ? std::to_string(int(type)) : "null")
        << ",\"dataTypeName\":" << (typeCode == OH_NN_SUCCESS ? Quote(TypeName(type)) : "null")
        << ",\"getShapeCode\":" << int(shapeCode) << ",\"reportedRank\":"
        << (shapeCode == OH_NN_SUCCESS ? std::to_string(rank) : "null") << ",\"shape\":";
    if (validShape) {
        out << '[';
        for (size_t i = 0; i < rank; ++i) { if (i) out << ','; out << shape[i]; }
        out << ']';
    } else { out << "null"; }
    out << ",\"getFormatCode\":" << int(formatCode) << ",\"format\":"
        << (formatCode == OH_NN_SUCCESS ? std::to_string(int(format)) : "null")
        << ",\"formatName\":" << (formatCode == OH_NN_SUCCESS ? Quote(FormatName(format)) : "null")
        << ",\"getElementCountCode\":" << int(elementCode) << ",\"elementCount\":"
        << (elementCode == OH_NN_SUCCESS ? std::to_string(elements) : "null")
        << ",\"getByteSizeCode\":" << int(bytesCode) << ",\"byteSize\":"
        << (bytesCode == OH_NN_SUCCESS ? std::to_string(bytes) : "null");
    const auto destroyCode = api.OH_NNTensorDesc_Destroy(&handle.descriptor);
    complete = complete && destroyCode == OH_NN_SUCCESS;
    out << ",\"destroyDescriptorCode\":" << int(destroyCode) << '}';
    return out.str();
}

std::string ModelMetadata(Api& api, const std::string& path, const Device& device,
                          const std::vector<uint8_t>* transformedBuffer = nullptr)
{
    ModelHandles handles(api);
    std::ostringstream out;
    out << "{\"path\":" << Quote(path) << ",\"deviceId\":" << device.id
        << ",\"deviceName\":" << Quote(device.name) << ",\"modelInputSemantics\":"
        << Quote(transformedBuffer ? "whole stock file with ROR1 on every byte; memory buffer only" :
                                     "unmodified stock file; does not reproduce Core AIFlowPass transform");
    handles.compilation = transformedBuffer ?
        api.OH_NNCompilation_ConstructWithOfflineModelBuffer(transformedBuffer->data(), transformedBuffer->size()) :
        api.OH_NNCompilation_ConstructWithOfflineModelFile(path.c_str());
    out << ",\"compilationReturnedNonNull\":" << (handles.compilation ? "true" : "false");
    if (!handles.compilation) return out.str() + ",\"status\":\"construct_failed\"}";
    const auto deviceCode = api.OH_NNCompilation_SetDevice(handles.compilation, device.id);
    out << ",\"setDeviceCode\":" << int(deviceCode);
    if (deviceCode != OH_NN_SUCCESS) return out.str() + ",\"status\":\"set_device_failed\"}";
    const auto buildCode = api.OH_NNCompilation_Build(handles.compilation);
    out << ",\"buildCode\":" << int(buildCode);
    if (buildCode != OH_NN_SUCCESS) return out.str() + ",\"status\":\"build_failed\"}";
    handles.executor = api.OH_NNExecutor_Construct(handles.compilation);
    out << ",\"executorReturnedNonNull\":" << (handles.executor ? "true" : "false");
    if (!handles.executor) return out.str() + ",\"status\":\"executor_failed\"}";

    size_t inputCount = 0, outputCount = 0;
    const auto inputCode = api.OH_NNExecutor_GetInputCount(handles.executor, &inputCount);
    const auto outputCode = api.OH_NNExecutor_GetOutputCount(handles.executor, &outputCount);
    const bool inputsValid = inputCode == OH_NN_SUCCESS && inputCount <= kMaxTensors;
    const bool outputsValid = outputCode == OH_NN_SUCCESS && outputCount <= kMaxTensors;
    bool complete = inputsValid && outputsValid;
    std::vector<std::string> inputs, outputs;
    if (inputsValid) {
        for (size_t i = 0; i < inputCount; ++i) inputs.push_back(TensorMetadata(api, handles.executor, true, i, complete));
    }
    if (outputsValid) {
        for (size_t i = 0; i < outputCount; ++i) outputs.push_back(TensorMetadata(api, handles.executor, false, i, complete));
    }
    out << ",\"getInputCountCode\":" << int(inputCode) << ",\"reportedInputCount\":"
        << (inputCode == OH_NN_SUCCESS ? std::to_string(inputCount) : "null")
        << ",\"getOutputCountCode\":" << int(outputCode) << ",\"reportedOutputCount\":"
        << (outputCode == OH_NN_SUCCESS ? std::to_string(outputCount) : "null")
        << ",\"inputs\":" << Array(inputs) << ",\"outputs\":" << Array(outputs)
        << ",\"status\":" << Quote(complete ? "metadata_complete" : "metadata_incomplete")
        << ",\"executorAndCompilationCleanup\":\"RAII before function return\"}";
    return out.str();
}
} // namespace

static std::string AuditFgModelMetadata(const std::vector<std::string>& modelPaths)
{
    static std::mutex auditMutex;
    std::lock_guard<std::mutex> lock(auditMutex);
    static Api api;
    std::ostringstream out;
    out << "{\"schemaVersion\":2,\"probe\":\"fg_offline_model_metadata\","
        "\"scope\":\"Public NNRT offline model build and declared tensor metadata only\","
        "\"inferenceExecuted\":false,\"modelBytesExported\":false,\"modelBytesPersisted\":false,"
        "\"fgCompatibilityEstablished\":false,\"outputSemanticsEstablished\":false,"
        "\"runtimeRetainedForProcessLifetime\":true,"
        "\"deviceSelection\":\"all enumerated OH_NN_ACCELERATOR devices; type does not establish NPU identity\","
        "\"rawFileFailureDoesNotRejectCoreTransformedBuffer\":true,"
        "\"provenance\":{"
        "\"integrationGuide\":\"https://developer.huawei.com/consumer/cn/doc/harmonyos-guides/cannkit-integration-model\","
        "\"helperApi\":\"https://developer.huawei.com/consumer/cn/doc/harmonyos-references/cannkit-hiai-helper-8h\","
        "\"sdkHeader\":\"CANNKit/hiai_helper.h\","
        "\"offlineBackendInOfficialGuide\":\"HIAI_F\","
        "\"registrationClaim\":\"dlopen registration is an observed comparison, not assumed from the guide\","
        "\"transformSource\":\"Static Core build ID 6f02b1af72476d69118daabbd4d630ca9cd5c696\","
        "\"transformOffsets\":\"whole-file read 0x116110..0x1161d4; ROR1 0xc6438..0xc64d0; buffer construct 0x100ab8\","
        "\"transform\":\"each byte = (byte >> 1) | (byte << 7), truncated to uint8; no added header/tail\"},"
        "\"library\":\"libneural_network_runtime.so\",\"libraryOpened\":"
        << (api.library ? "true" : "false") << ",\"loaderError\":"
        << (api.loaderError.empty() ? "null" : Quote(api.loaderError))
        << ",\"missingSymbols\":" << Array(api.missingSymbols);
    if (!api.library || !api.missingSymbols.empty()) return out.str() + ",\"status\":\"runtime_unavailable\"}";
    if (modelPaths.empty() || modelPaths.size() > kMaxModels ||
        std::any_of(modelPaths.begin(), modelPaths.end(), [](const std::string& path) {
            return path.empty() || path[0] != '/' || path.find('\0') != std::string::npos;
        })) return out.str() + ",\"status\":\"invalid_absolute_model_paths\"}";

    const auto before = EnumerateDevices(api);
    out << ",\"devicesBeforeCANNLoad\":" << before.json();
    dlerror();
    void* existingHiai = dlopen("libhiai_foundation.so", RTLD_NOW | RTLD_NOLOAD);
    const bool alreadyResident = existingHiai != nullptr;
    if (existingHiai) dlclose(existingHiai); // Drop only the temporary NOLOAD reference.
    static HiaiApi hiai;
    const auto afterLoad = EnumerateDevices(api);
    out << ",\"cannBackend\":{\"library\":\"libhiai_foundation.so\","
        "\"flags\":\"RTLD_NOW|RTLD_LOCAL\",\"residentBeforeLoad\":"
        << (alreadyResident ? "true" : "false") << ",\"libraryOpened\":"
        << (hiai.library ? "true" : "false") << ",\"retainedForProcessLifetime\":"
        << (hiai.library ? "true" : "false") << ",\"loaderError\":"
        << (hiai.loaderError.empty() ? "null" : Quote(hiai.loaderError))
        << ",\"missingSymbols\":" << Array(hiai.missingSymbols) << '}'
        << ",\"devicesAfterCANNLoadBeforePublicCalls\":" << afterLoad.json();

    const char* version = hiai.getVersion ? hiai.getVersion() : nullptr;
    out << ",\"cannVersion\":" << (version ? Quote(version) : "null");
    std::vector<std::string> compatibilityRecords;
    std::vector<TransformedModel> transformedModels;
    transformedModels.reserve(modelPaths.size());
    for (const auto& path : modelPaths) {
        transformedModels.emplace_back();
        auto& transformed = transformedModels.back();
        ReadAndTransform(path, transformed);
        const bool boundedReadComplete = transformed.readStatus == "whole_file_read_and_ror1_applied_in_memory";
        const bool fileCheckCalled = boundedReadComplete && hiai.checkFile;
        std::ostringstream compatibility;
        compatibility << "{\"path\":" << Quote(path) << ",\"checkFromFileCalled\":"
            << (fileCheckCalled ? "true" : "false");
        if (fileCheckCalled) {
            const auto code = hiai.checkFile(path.c_str());
            compatibility << ",\"checkFromFileResult\":" << int(code);
        }
        compatibility << ",\"fileCheckScope\":\"raw stock file only; result 1 also covers check failure\"";
        if (boundedReadComplete && hiai.checkBuffer) {
            transformed.compatibility = hiai.checkBuffer(transformed.bytes.data(), transformed.bytes.size());
            transformed.compatibilityCalled = true;
        }
        compatibility << ",\"transformedBufferReadStatus\":" << Quote(transformed.readStatus)
            << ",\"readErrno\":" << transformed.readError << ",\"reportedFileSize\":" << transformed.fileSize
            << ",\"maxBytesExclusive\":" << kMaxModelBytesExclusive
            << ",\"checkFromBufferCalled\":" << (transformed.compatibilityCalled ? "true" : "false")
            << ",\"checkFromBufferResult\":"
            << (transformed.compatibilityCalled ? std::to_string(int(transformed.compatibility)) : "null")
            << ",\"bufferCheckScope\":\"in-memory whole-file ROR1; not inference or FG compatibility\"}";
        compatibilityRecords.push_back(compatibility.str());
    }
    const auto afterChecks = EnumerateDevices(api);
    out << ",\"modelCompatibility\":" << Array(compatibilityRecords)
        << ",\"devicesAfterPublicCompatibilityChecks\":" << afterChecks.json();
    if (!afterChecks.valid) return out.str() + ",\"status\":\"device_enumeration_failed\"}";

    std::vector<std::string> devices, models, transformedMetadata;
    size_t selectedCount = 0;
    bool cannOfflineBackendObserved = false;
    for (const auto& device : afterChecks.devices) {
        devices.push_back(device.json());
        if (!device.selected()) continue;
        cannOfflineBackendObserved = cannOfflineBackendObserved || device.name == "HIAI_F";
        ++selectedCount;
        for (size_t index = 0; index < modelPaths.size(); ++index) {
            const auto& path = modelPaths[index];
            const auto& transformed = transformedModels[index];
            if (transformed.readStatus == "whole_file_read_and_ror1_applied_in_memory") {
                models.push_back(ModelMetadata(api, path, device));
            } else {
                models.push_back("{\"path\":" + Quote(path) + ",\"deviceId\":" +
                    std::to_string(device.id) + ",\"deviceName\":" + Quote(device.name) +
                    ",\"status\":\"stock_file_size_or_read_not_qualified_no_build\"}");
            }
            if (transformed.compatibilityCalled && transformed.compatibility == HIAI_COMPATIBILITY_COMPATIBLE) {
                transformedMetadata.push_back(ModelMetadata(api, path, device, &transformed.bytes));
            } else {
                transformedMetadata.push_back("{\"path\":" + Quote(path) + ",\"deviceId\":" +
                    std::to_string(device.id) + ",\"deviceName\":" + Quote(device.name) +
                    ",\"status\":\"transformed_buffer_compatibility_not_confirmed_no_build\"}");
            }
        }
    }
    out << ",\"devices\":" << Array(devices) << ",\"selectedAcceleratorCount\":" << selectedCount
        << ",\"cannOfflineBackendObserved\":" << (cannOfflineBackendObserved ? "true" : "false")
        << ",\"models\":" << Array(models) << ",\"transformedModels\":" << Array(transformedMetadata)
        << ",\"transformedBytesErasedBeforeReturn\":true,\"status\":"
        << Quote(selectedCount ? "audit_finished_inspect_each_model_status" : "no_accelerator_no_build") << '}';
    return out.str();
}
std::string inspectSystemModelMetadata()
{
    return AuditFgModelMetadata({
        "/sys_prod/etc/graphics_game/ai_model/aifi1.om",
        "/sys_prod/etc/graphics_game/ai_model/aifi2.om"
    });
}
} // namespace muguet::fg_probe
