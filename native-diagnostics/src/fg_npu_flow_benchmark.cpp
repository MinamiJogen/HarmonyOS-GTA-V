#include "fg_npu_flow_benchmark.h"

#include <CANNKit/hiai_helper.h>
#include <CANNKit/hiai_options.h>
#include <neural_network_runtime/neural_network_core.h>

#include <algorithm>
#include <array>
#include <cerrno>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <dlfcn.h>
#include <fcntl.h>
#include <fstream>
#include <limits>
#include <mutex>
#include <sstream>
#include <stdexcept>
#include <sys/stat.h>
#include <unistd.h>
#include <vector>

namespace muguet::fg_probe {
namespace {
constexpr int kWidth = 448;
constexpr int kHeight = 208;
constexpr size_t kPixels = size_t(kWidth) * kHeight;
constexpr size_t kInputBytes = 4 * kPixels;
constexpr size_t kFlowElements = 6 * kPixels;
constexpr size_t kFlowBytes = 2 * kFlowElements;
constexpr size_t kMaxModelBytesExclusive = 8 * 1024 * 1024;
constexpr int kWarmRuns = 2;
constexpr int kTimedRuns = 5;
constexpr int kMetricMargin = 24;
constexpr char kModelPath[] = "/sys_prod/etc/graphics_game/ai_model/aifi2.om";
using Clock = std::chrono::steady_clock;

std::string Quote(const std::string& text)
{
    constexpr char hex[] = "0123456789abcdef";
    std::string out = "\"";
    for (unsigned char c : text) {
        if (c == '"' || c == '\\') { out += '\\'; out += char(c); }
        else if (c < 32) { out += "\\u00"; out += hex[c >> 4]; out += hex[c & 15]; }
        else { out += char(c); }
    }
    return out + '"';
}

std::string Array(const std::vector<std::string>& items)
{
    std::string out = "[";
    for (size_t i = 0; i < items.size(); ++i) { if (i) out += ','; out += items[i]; }
    return out + ']';
}

double Milliseconds(Clock::time_point from, Clock::time_point to)
{
    return std::chrono::duration<double, std::milli>(to - from).count();
}

std::string Number(double value)
{
    if (!std::isfinite(value)) return "null";
    std::ostringstream out;
    out.precision(10);
    out << value;
    return out.str();
}

struct Report {
    std::string directory;
    std::vector<std::string> setup, cases;
    void event(const std::string& json) const
    {
        if (directory.empty()) return;
        std::ofstream stream(directory + "/events.jsonl", std::ios::app);
        stream << json << '\n';
        stream.flush();
        if (!stream) throw std::runtime_error("Cannot persist benchmark event");
    }
    void step(const std::string& json) { setup.push_back(json); event(json); }
    std::string finish(const std::string& status, const std::string& error = "") const
    {
        std::ostringstream out;
        out << "{\"schemaVersion\":1,\"probe\":\"public_nnrt_flow_cpu_warp\",\"pid\":" << getpid()
            << ",\"status\":" << Quote(status) << ",\"error\":" << (error.empty() ? "null" : Quote(error))
            << ",\"directory\":" << (directory.empty() ? "null" : Quote(directory))
            << ",\"modelPath\":" << Quote(kModelPath)
            << ",\"modelBytesExported\":false,\"modelBytesPersisted\":false,"
            "\"privateApiCalled\":false,\"vendorShaderExecuted\":false,"
            "\"npuExecutionProven\":false,\"presentationTested\":false,\"output30FpsQualified\":false,"
            "\"candidateIsProductionWinner\":false,\"width\":" << kWidth << ",\"height\":" << kHeight
            << ",\"warmupPerCase\":" << kWarmRuns << ",\"timedRunsPerCase\":" << kTimedRuns
            << ",\"metricMarginPixels\":" << kMetricMargin
            << ",\"metricUnits\":\"MAE over RGB bytes, range 0..255; interior ROI only\","
            "\"formulaEvidence\":{\"source\":\"Static Core build ID 6f02b1af72476d69118daabbd4d630ca9cd5c696\","
            "\"transformShaderOffset\":\"0x664bc\",\"resolveShaderOffset\":\"0x66de8\","
            "\"implementedBy\":\"Independent CPU bilinear sampler and blend; no vendor shader loaded\","
            "\"formula\":\"w=sigmoid(ch4); f=sigmoid(ch5); mixed=w*A(uv+flowA)+(1-w)*B(uv+flowB); out=f*B(uv)+(1-f)*mixed\","
            "\"uncertainties\":\"Public metadata does not define packed/planar color encoding, flow-pair frame assignment or XY/YX component axes. Explicit candidates test these assumptions. Model inference/training correctness and quality are not established.\"},"
            "\"setup\":" << Array(setup) << ",\"cases\":" << Array(cases) << '}';
        const std::string json = out.str();
        if (!directory.empty()) {
            std::ofstream stream(directory + "/result.json");
            stream << json << '\n';
            stream.close();
            if (!stream) return json.substr(0, json.size() - 1) + ",\"resultFileWriteFailed\":true}";
        }
        return json;
    }
};

struct Api {
    // Public runtimes remain mapped until process exit; driver threads can
    // outlive executor destruction. No dlclose of these retained references.
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
        if (!name) { error += std::string(error.empty() ? "" : "; ") + "Missing " #name; }
#define FG_FLOW_LOAD_HIAI(name) \
        name = reinterpret_cast<decltype(name)>(dlsym(hiai, #name)); \
        if (!name) { error += std::string(error.empty() ? "" : "; ") + "Missing " #name; }
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
    if (file.value < 0) throw std::runtime_error("Stock model open failed, errno=" + std::to_string(errno));
    struct stat metadata{};
    if (fstat(file.value, &metadata) != 0) throw std::runtime_error("Stock model fstat failed");
    if (!S_ISREG(metadata.st_mode) || metadata.st_size <= 0 || uint64_t(metadata.st_size) >= kMaxModelBytesExclusive)
        throw std::runtime_error("Stock model must be regular with 0<size<8MiB");
    model.bytes.resize(size_t(metadata.st_size));
    size_t offset = 0;
    while (offset < model.bytes.size()) {
        const ssize_t count = read(file.value, model.bytes.data() + offset, model.bytes.size() - offset);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) throw std::runtime_error("Stock model short/failed read");
        offset += size_t(count);
    }
    uint8_t extra = 0;
    ssize_t count;
    do { count = read(file.value, &extra, 1); } while (count < 0 && errno == EINTR);
    if (count != 0) throw std::runtime_error("Stock model size changed or EOF check failed");
    for (auto& byte : model.bytes) byte = uint8_t((byte >> 1) | (byte << 7));
}

struct Handles {
    Api& api;
    OH_NNCompilation* compilation = nullptr;
    OH_NNExecutor* executor = nullptr;
    std::array<NN_Tensor*, 2> inputs{{nullptr, nullptr}}, outputs{{nullptr, nullptr}};
    ~Handles()
    {
        // No RunAsync is used. Every submitted RunSync has returned before cleanup.
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

size_t SelectDevice(Api& api, Report& report)
{
    const size_t* ids = nullptr;
    uint32_t count = 0;
    const auto code = api.OH_NNDevice_GetAllDevicesID(&ids, &count);
    report.step("{\"stage\":\"enumerate_devices\",\"code\":" + std::to_string(int(code)) +
                ",\"count\":" + std::to_string(count) + '}');
    if (code != OH_NN_SUCCESS || !ids || !count || count > 32) throw std::runtime_error("Device enumeration failed");
    const std::vector<size_t> copied(ids, ids + count);
    size_t selected = 0;
    bool found = false;
    for (size_t id : copied) {
        const char* name = nullptr;
        OH_NN_DeviceType type = OH_NN_OTHERS;
        const auto nameCode = api.OH_NNDevice_GetName(id, &name);
        const auto typeCode = api.OH_NNDevice_GetType(id, &type);
        const bool match = nameCode == OH_NN_SUCCESS && name && std::strcmp(name, "HIAI_F") == 0 &&
                           typeCode == OH_NN_SUCCESS && type == OH_NN_ACCELERATOR;
        report.step("{\"stage\":\"device\",\"id\":" + std::to_string(id) + ",\"nameCode\":" +
                    std::to_string(int(nameCode)) + ",\"name\":" + (name ? Quote(name) : "null") +
                    ",\"typeCode\":" + std::to_string(int(typeCode)) + ",\"type\":" +
                    std::to_string(int(type)) + ",\"matchesHiaiF\":" + (match ? "true" : "false") + '}');
        if (match && !found) { selected = id; found = true; }
    }
    if (!found) throw std::runtime_error("Public HIAI_F accelerator was not observed");
    return selected;
}

void* AllocateTensor(Api& api, Handles& handles, size_t deviceId, bool input, size_t index, Report& report)
{
    Descriptor descriptor{api, input ? api.OH_NNExecutor_CreateInputTensorDesc(handles.executor, index) :
                                      api.OH_NNExecutor_CreateOutputTensorDesc(handles.executor, index)};
    if (!descriptor.value) throw std::runtime_error("Tensor descriptor creation failed");
    const char* name = nullptr;
    int32_t* shape = nullptr;
    size_t rank = 0, bytes = 0;
    OH_NN_DataType type = OH_NN_UNKNOWN;
    OH_NN_Format format = OH_NN_FORMAT_NONE;
    const auto nameCode = api.OH_NNTensorDesc_GetName(descriptor.value, &name);
    const auto shapeCode = api.OH_NNTensorDesc_GetShape(descriptor.value, &shape, &rank);
    const auto typeCode = api.OH_NNTensorDesc_GetDataType(descriptor.value, &type);
    const auto formatCode = api.OH_NNTensorDesc_GetFormat(descriptor.value, &format);
    const auto bytesCode = api.OH_NNTensorDesc_GetByteSize(descriptor.value, &bytes);
    const std::array<int32_t, 4> expected = input ? std::array<int32_t, 4>{{1, 4, kHeight, kWidth}} :
        (index == 0 ? std::array<int32_t, 4>{{1, 6, kHeight, kWidth}} : std::array<int32_t, 4>{{1, 2, 1, 1}});
    const auto expectedType = input ? OH_NN_UINT8 : (index == 0 ? OH_NN_FLOAT16 : OH_NN_FLOAT32);
    const size_t expectedBytes = input ? kInputBytes : (index == 0 ? kFlowBytes : 2 * sizeof(float));
    const bool shapeReadable = shapeCode == OH_NN_SUCCESS && shape && rank <= 16;
    std::ostringstream item;
    item << "{\"stage\":\"tensor_descriptor\",\"input\":" << (input ? "true" : "false")
        << ",\"index\":" << index << ",\"nameCode\":" << int(nameCode) << ",\"name\":"
        << (nameCode == OH_NN_SUCCESS && name ? Quote(name) : "null")
        << ",\"shapeCode\":" << int(shapeCode) << ",\"rank\":" << rank << ",\"shape\":";
    if (shapeReadable) {
        item << '[';
        for (size_t i = 0; i < rank; ++i) { if (i) item << ','; item << shape[i]; }
        item << ']';
    } else { item << "null"; }
    item << ",\"typeCode\":" << int(typeCode) << ",\"dataType\":" << int(type)
        << ",\"formatCode\":" << int(formatCode) << ",\"format\":" << int(format)
        << ",\"byteSizeCode\":" << int(bytesCode) << ",\"byteSize\":" << bytes << '}';
    report.step(item.str());
    if (!shapeReadable || rank != expected.size() || !std::equal(expected.begin(), expected.end(), shape) ||
        typeCode != OH_NN_SUCCESS || type != expectedType || formatCode != OH_NN_SUCCESS ||
        format != OH_NN_FORMAT_NCHW || bytesCode != OH_NN_SUCCESS || bytes != expectedBytes)
        throw std::runtime_error("Actual model tensor does not match the audited aifi2 fixed contract");
    NN_Tensor*& tensor = input ? handles.inputs[index] : handles.outputs[index];
    tensor = api.OH_NNTensor_Create(deviceId, descriptor.value);
    if (!tensor) throw std::runtime_error("Public tensor allocation failed");
    size_t mappedBytes = 0;
    const auto sizeCode = api.OH_NNTensor_GetSize(tensor, &mappedBytes);
    void* data = api.OH_NNTensor_GetDataBuffer(tensor);
    report.step("{\"stage\":\"tensor_memory\",\"input\":" + std::string(input ? "true" : "false") +
                ",\"index\":" + std::to_string(index) + ",\"sizeCode\":" + std::to_string(int(sizeCode)) +
                ",\"mappedBytes\":" + std::to_string(mappedBytes) + ",\"mappedNonNull\":" +
                (data ? "true" : "false") + '}');
    if (sizeCode != OH_NN_SUCCESS || mappedBytes != expectedBytes || !data)
        throw std::runtime_error("Tensor mapping or exact mapped byte size check failed");
    return data;
}

using Image = std::vector<uint8_t>; // Interleaved RGBA, alpha fixed to 255.
using RgbImage = std::vector<float>; // Interleaved RGB in 0..255 units.

int Wrap(int value, int limit) { value %= limit; return value < 0 ? value + limit : value; }

Image SyntheticImage(int shiftX, int shiftY)
{
    Image image(kInputBytes);
    for (int y = 0; y < kHeight; ++y) {
        for (int x = 0; x < kWidth; ++x) {
            const int u = Wrap(x - shiftX, kWidth), v = Wrap(y - shiftY, kHeight);
            const int checker = ((u / 19) ^ (v / 17)) & 1;
            const int circle = (u - 270) * (u - 270) + (v - 105) * (v - 105) < 48 * 48 ? 79 : 0;
            const size_t index = (size_t(y) * kWidth + x) * 4;
            image[index] = uint8_t((u * 3 + v * 5 + checker * 91) & 255);
            image[index + 1] = uint8_t((u * 7 + v * 2 + circle) & 255);
            image[index + 2] = uint8_t((checker * 123 + ((u * 11) ^ (v * 17))) & 255);
            image[index + 3] = 255;
        }
    }
    return image;
}

void PackInput(const Image& image, bool planar, void* destination)
{
    auto* output = static_cast<uint8_t*>(destination);
    if (!planar) { std::memcpy(output, image.data(), image.size()); return; }
    for (size_t pixel = 0; pixel < kPixels; ++pixel)
        for (size_t channel = 0; channel < 4; ++channel)
            output[channel * kPixels + pixel] = image[pixel * 4 + channel];
}

float HalfToFloat(uint16_t bits)
{
    const int exponent = (bits >> 10) & 31;
    const int fraction = bits & 1023;
    float value;
    if (exponent == 0) value = std::ldexp(float(fraction), -24);
    else if (exponent == 31) value = fraction ? std::numeric_limits<float>::quiet_NaN() :
                                               std::numeric_limits<float>::infinity();
    else value = std::ldexp(1.0f + float(fraction) / 1024.0f, exponent - 15);
    return bits & 0x8000 ? -value : value;
}

float Sigmoid(float value)
{
    if (value >= 0) return 1.0f / (1.0f + std::exp(-value));
    const float exponent = std::exp(value);
    return exponent / (1.0f + exponent);
}

float Sample(const Image& image, float x, float y, size_t channel)
{
    x = std::clamp(x, 0.0f, float(kWidth - 1));
    y = std::clamp(y, 0.0f, float(kHeight - 1));
    const int x0 = int(std::floor(x)), y0 = int(std::floor(y));
    const int x1 = std::min(x0 + 1, kWidth - 1), y1 = std::min(y0 + 1, kHeight - 1);
    const float wx = x - x0, wy = y - y0;
    const auto read = [&](int u, int v) { return float(image[(size_t(v) * kWidth + u) * 4 + channel]); };
    return (1 - wy) * ((1 - wx) * read(x0, y0) + wx * read(x1, y0)) +
           wy * ((1 - wx) * read(x0, y1) + wx * read(x1, y1));
}

RgbImage Warp(const Image& first, const Image& second, const std::vector<float>& flow, bool swapPairs, bool swapAxes)
{
    RgbImage result(kPixels * 3);
    const size_t firstPlane = swapPairs ? 2 : 0, secondPlane = swapPairs ? 0 : 2;
    for (int y = 0; y < kHeight; ++y) {
        for (int x = 0; x < kWidth; ++x) {
            const size_t pixel = size_t(y) * kWidth + x;
            const float ax = flow[(firstPlane + (swapAxes ? 1 : 0)) * kPixels + pixel];
            const float ay = flow[(firstPlane + (swapAxes ? 0 : 1)) * kPixels + pixel];
            const float bx = flow[(secondPlane + (swapAxes ? 1 : 0)) * kPixels + pixel];
            const float by = flow[(secondPlane + (swapAxes ? 0 : 1)) * kPixels + pixel];
            const float weight = Sigmoid(flow[4 * kPixels + pixel]);
            const float fallback = Sigmoid(flow[5 * kPixels + pixel]);
            for (size_t c = 0; c < 3; ++c) {
                const float mixed = weight * Sample(first, x + ax, y + ay, c) +
                                    (1 - weight) * Sample(second, x + bx, y + by, c);
                const float value = fallback * second[pixel * 4 + c] + (1 - fallback) * mixed;
                result[pixel * 3 + c] = std::clamp(value, 0.0f, 255.0f);
            }
        }
    }
    return result;
}

RgbImage Rgb(const Image& image)
{
    RgbImage result(kPixels * 3);
    for (size_t p = 0; p < kPixels; ++p)
        for (size_t c = 0; c < 3; ++c) result[p * 3 + c] = image[p * 4 + c];
    return result;
}

double Mae(const RgbImage& first, const RgbImage& second)
{
    double error = 0;
    size_t count = 0;
    for (int y = kMetricMargin; y < kHeight - kMetricMargin; ++y)
        for (int x = kMetricMargin; x < kWidth - kMetricMargin; ++x)
            for (size_t c = 0; c < 3; ++c) {
                const size_t index = (size_t(y) * kWidth + x) * 3 + c;
                error += std::abs(double(first[index]) - second[index]);
                ++count;
            }
    return error / count;
}

void SavePpm(const std::string& path, const RgbImage& image)
{
    std::ofstream output(path, std::ios::binary);
    output << "P6\n" << kWidth << ' ' << kHeight << "\n255\n";
    std::vector<uint8_t> pixels(image.size());
    for (size_t i = 0; i < image.size(); ++i) pixels[i] = uint8_t(std::lround(std::clamp(image[i], 0.0f, 255.0f)));
    output.write(reinterpret_cast<const char*>(pixels.data()), std::streamsize(pixels.size()));
    output.close();
    if (!output) throw std::runtime_error("Cannot persist generated PPM: " + path);
}

std::string FlowStats(const std::vector<float>& flow, bool& finite)
{
    std::vector<std::string> planes;
    finite = true;
    for (size_t channel = 0; channel < 6; ++channel) {
        float low = std::numeric_limits<float>::infinity(), high = -low;
        double total = 0;
        size_t invalid = 0;
        for (size_t p = 0; p < kPixels; ++p) {
            const float value = flow[channel * kPixels + p];
            if (!std::isfinite(value)) { ++invalid; continue; }
            low = std::min(low, value); high = std::max(high, value); total += value;
        }
        finite = finite && invalid == 0;
        planes.push_back("{\"plane\":" + std::to_string(channel) + ",\"nonFinite\":" +
            std::to_string(invalid) + ",\"min\":" + Number(low) + ",\"max\":" + Number(high) +
            ",\"mean\":" + (invalid == kPixels ? "null" : Number(total / (kPixels - invalid))) + '}');
    }
    return Array(planes);
}

std::string RunCase(Api& api, Handles& handles, const std::array<void*, 2>& inputMemory,
                    const std::array<void*, 2>& outputMemory, bool planar, bool identity, Report& report)
{
    const std::string packing = planar ? "planar_rgba_uint8" : "packed_rgba_uint8";
    const std::string name = packing + (identity ? "_identity" : "_translation_dx8_dy4");
    const Image first = SyntheticImage(0, 0), second = SyntheticImage(identity ? 0 : 8, identity ? 0 : 4);
    const Image groundTruth = SyntheticImage(identity ? 0 : 4, identity ? 0 : 2);
    const auto packBegin = Clock::now();
    PackInput(first, planar, inputMemory[0]);
    PackInput(second, planar, inputMemory[1]);
    const double packingMs = Milliseconds(packBegin, Clock::now());
    const auto ground = Rgb(groundTruth), repeat = Rgb(second), firstRgb = Rgb(first);
    RgbImage blend(kPixels * 3);
    for (size_t i = 0; i < blend.size(); ++i) blend[i] = 0.5f * (firstRgb[i] + repeat[i]);
    SavePpm(report.directory + '/' + name + "_img0.ppm", firstRgb);
    SavePpm(report.directory + '/' + name + "_img1.ppm", repeat);
    SavePpm(report.directory + '/' + name + "_gt.ppm", ground);
    SavePpm(report.directory + '/' + name + "_blend.ppm", blend);
    std::vector<float> flow(kFlowElements);
    std::vector<std::string> runs;
    std::vector<double> timed;
    std::string lastStats;
    for (int iteration = 0; iteration < kWarmRuns + kTimedRuns; ++iteration) {
        const bool warmup = iteration < kWarmRuns;
        // A success code with unwritten output must fail the finite guards.
        auto* halfOutput = static_cast<uint16_t*>(outputMemory[0]);
        std::fill(halfOutput, halfOutput + kFlowElements, uint16_t(0x7e00));
        const std::array<float, 2> sentinel{{std::numeric_limits<float>::quiet_NaN(), std::numeric_limits<float>::quiet_NaN()}};
        std::memcpy(outputMemory[1], sentinel.data(), sizeof(sentinel));
        const std::string context = "\"case\":" + Quote(name) + ",\"iteration\":" + std::to_string(iteration) +
                                    ",\"warmup\":" + (warmup ? "true" : "false");
        report.event("{\"stage\":\"run_before\"," + context + '}');
        const auto begin = Clock::now();
        const auto code = api.OH_NNExecutor_RunSync(handles.executor, handles.inputs.data(), handles.inputs.size(),
                                                   handles.outputs.data(), handles.outputs.size());
        const double elapsedMs = Milliseconds(begin, Clock::now());
        report.event("{\"stage\":\"run_after\"," + context + ",\"code\":" + std::to_string(int(code)) +
                     ",\"nnrtMs\":" + Number(elapsedMs) + '}');
        if (code != OH_NN_SUCCESS) throw std::runtime_error(name + " public RunSync failed, code=" + std::to_string(int(code)));
        const auto readBegin = Clock::now();
        for (size_t i = 0; i < flow.size(); ++i) flow[i] = HalfToFloat(halfOutput[i]);
        std::array<float, 2> logits{};
        std::memcpy(logits.data(), outputMemory[1], sizeof(logits));
        bool finite = false;
        lastStats = FlowStats(flow, finite);
        finite = finite && std::isfinite(logits[0]) && std::isfinite(logits[1]);
        const float maximum = std::max(logits[0], logits[1]);
        const double e0 = std::exp(double(logits[0] - maximum)), e1 = std::exp(double(logits[1] - maximum));
        const std::string run = "{" + context + ",\"runSyncCode\":" + std::to_string(int(code)) +
            ",\"nnrtMs\":" + Number(elapsedMs) + ",\"outputReadConvertStatsMs\":" + Number(Milliseconds(readBegin, Clock::now())) +
            ",\"allOutputsFinite\":" + (finite ? "true" : "false") + ",\"output1Fp32\":[" +
            Number(logits[0]) + ',' + Number(logits[1]) + "],\"output1SoftmaxSecond\":" + Number(e1 / (e0 + e1)) +
            ",\"output1GateApplied\":false,\"flowPlanes\":" + lastStats + '}';
        runs.push_back(run);
        report.event(run);
        if (!finite) throw std::runtime_error(name + " non-finite or unwritten output detected");
        if (!warmup) timed.push_back(elapsedMs);
    }
    std::vector<std::string> candidates;
    std::vector<RgbImage> warped;
    for (bool swapAxes : {false, true}) {
        for (bool swapPairs : {false, true}) {
            const std::string candidate = std::string(swapAxes ? "yx" : "xy") +
                (swapPairs ? "_flow23_to_img0_flow01_to_img1" : "_flow01_to_img0_flow23_to_img1");
            const auto begin = Clock::now();
            auto result = Warp(first, second, flow, swapPairs, swapAxes);
            const double warpMs = Milliseconds(begin, Clock::now());
            const std::string filename = name + '_' + candidate + ".ppm";
            SavePpm(report.directory + '/' + filename, result);
            candidates.push_back("{\"name\":" + Quote(candidate) + ",\"interpretation\":\"candidate only; plus displacement in model-grid pixel units\","
                "\"cpuWarpMs\":" + Number(warpMs) + ",\"maeVsGt\":" + Number(Mae(result, ground)) +
                ",\"maeVsRepeat\":" + Number(Mae(result, repeat)) + ",\"maeVsBlend\":" + Number(Mae(result, blend)) +
                ",\"ppm\":" + Quote(filename) + '}');
            warped.push_back(std::move(result));
        }
    }
    std::vector<std::string> differences;
    for (size_t firstIndex = 0; firstIndex < warped.size(); ++firstIndex)
        for (size_t secondIndex = firstIndex + 1; secondIndex < warped.size(); ++secondIndex)
            differences.push_back("{\"candidateA\":" + std::to_string(firstIndex) + ",\"candidateB\":" +
                std::to_string(secondIndex) + ",\"mae\":" + Number(Mae(warped[firstIndex], warped[secondIndex])) + '}');
    double sum = 0;
    for (double value : timed) sum += value;
    auto sorted = timed;
    std::sort(sorted.begin(), sorted.end());
    std::ostringstream out;
    out << "{\"name\":" << Quote(name) << ",\"status\":\"offscreen_case_completed\",\"packing\":" << Quote(packing)
        << ",\"packingRule\":" << Quote(planar ? "input[c*H*W+y*W+x] = RGBA[y,x,c]" : "input[(y*W+x)*4+c] = RGBA[y,x,c]")
        << ",\"descriptorFormatRemains\":\"NCHW; packed byte interpretation is an explicit candidate\","
        "\"input0\":\"texture shift (0,0)\",\"input1Shift\":[" << (identity ? 0 : 8) << ',' << (identity ? 0 : 4)
        << "],\"gtShift\":[" << (identity ? 0 : 4) << ',' << (identity ? 0 : 2)
        << "],\"textureVersion\":1,\"textureBoundary\":\"periodic; metric excludes 24px border\","
        "\"packingMs\":" << Number(packingMs) << ",\"repeatMaeVsGt\":" << Number(Mae(repeat, ground))
        << ",\"blendMaeVsGt\":" << Number(Mae(blend, ground)) << ",\"nnrtTimedMeanMs\":" << Number(sum / timed.size())
        << ",\"nnrtTimedMedianMs\":" << Number(sorted[sorted.size() / 2])
        << ",\"nnrtTimedMinMs\":" << Number(sorted.front()) << ",\"nnrtTimedMaxMs\":" << Number(sorted.back())
        << ",\"runs\":" << Array(runs) << ",\"warpCandidates\":" << Array(candidates)
        << ",\"candidateDifferences\":" << Array(differences) << '}';
    return out.str();
}

std::string Execute(const std::string& filesDir, Report& report)
{
    if (filesDir.empty() || filesDir[0] != '/' || filesDir.find('\0') != std::string::npos)
        throw std::runtime_error("Absolute app filesDir required");
    struct stat directory{};
    if (stat(filesDir.c_str(), &directory) != 0 || !S_ISDIR(directory.st_mode))
        throw std::runtime_error("App filesDir is not an existing directory");
    std::string pattern = filesDir + "/fg-npu-flow-XXXXXX";
    std::vector<char> path(pattern.begin(), pattern.end()); path.push_back('\0');
    char* created = mkdtemp(path.data());
    if (!created) throw std::runtime_error("Benchmark directory creation failed");
    report.directory = created;
    report.step("{\"stage\":\"begin\",\"scope\":\"generated-image offscreen inference and independent CPU warp only\"}");
    static Api api;
    report.step("{\"stage\":\"public_runtime_load\",\"nnrtOpened\":" + std::string(api.nnrt ? "true" : "false") +
                ",\"cannOpened\":" + (api.hiai ? "true" : "false") + ",\"retainedForProcessLifetime\":true,\"error\":" +
                (api.error.empty() ? "null" : Quote(api.error)) + '}');
    if (!api.error.empty()) throw std::runtime_error(api.error);
    const char* version = api.HMS_HiAI_GetVersion();
    report.step("{\"stage\":\"cann_version\",\"version\":" + (version ? Quote(version) : "null") + '}');
    const size_t deviceId = SelectDevice(api, report);
    ModelBytes model; // Outlives all tensor/executor/compilation handles below.
    ReadModel(model);
    const auto compatibility = api.HMS_HiAICompatibility_CheckFromBuffer(model.bytes.data(), model.bytes.size());
    report.step("{\"stage\":\"transformed_model_compatibility\",\"wholeFileRor1\":true,\"byteSize\":" +
                std::to_string(model.bytes.size()) + ",\"result\":" + std::to_string(int(compatibility)) + '}');
    if (compatibility != HIAI_COMPATIBILITY_COMPATIBLE) throw std::runtime_error("Transformed model compatibility not confirmed");
    Handles handles{api};
    handles.compilation = api.OH_NNCompilation_ConstructWithOfflineModelBuffer(model.bytes.data(), model.bytes.size());
    if (!handles.compilation) throw std::runtime_error("Public model compilation construction failed");
    const auto deviceCode = api.OH_NNCompilation_SetDevice(handles.compilation, deviceId);
    HiAI_ExecuteDevice requestedDevice = HIAI_EXECUTE_DEVICE_NPU;
    const auto orderCode = api.HMS_HiAIOptions_SetModelDeviceOrder(handles.compilation, &requestedDevice, 1);
    const auto fallbackCode = api.HMS_HiAIOptions_SetFallbackMode(handles.compilation, HIAI_FALLBACK_DISABLED);
    const size_t orderCount = api.HMS_HiAIOptions_GetModelDeviceCount(handles.compilation);
    const auto* order = api.HMS_HiAIOptions_GetModelDeviceOrder(handles.compilation);
    const auto fallback = api.HMS_HiAIOptions_GetFallbackMode(handles.compilation);
    report.step("{\"stage\":\"npu_options\",\"deviceId\":" + std::to_string(deviceId) +
                ",\"setDeviceCode\":" + std::to_string(int(deviceCode)) + ",\"setModelDeviceOrderCode\":" +
                std::to_string(int(orderCode)) + ",\"setFallbackModeCode\":" + std::to_string(int(fallbackCode)) +
                ",\"readbackDeviceCount\":" + std::to_string(orderCount) + ",\"readbackFirstDevice\":" +
                (order && orderCount ? std::to_string(int(order[0])) : "null") + ",\"readbackFallback\":" +
                std::to_string(int(fallback)) + ",\"actualNpuPlacementProven\":false}");
    if (deviceCode != OH_NN_SUCCESS || orderCode != OH_NN_SUCCESS || fallbackCode != OH_NN_SUCCESS ||
        orderCount != 1 || !order || order[0] != HIAI_EXECUTE_DEVICE_NPU || fallback != HIAI_FALLBACK_DISABLED)
        throw std::runtime_error("Public NPU-only/fallback-disabled configuration was not confirmed");
    report.event("{\"stage\":\"build_before\"}");
    const auto buildBegin = Clock::now();
    const auto buildCode = api.OH_NNCompilation_Build(handles.compilation);
    report.step("{\"stage\":\"build_after\",\"code\":" + std::to_string(int(buildCode)) +
                ",\"elapsedMs\":" + Number(Milliseconds(buildBegin, Clock::now())) + '}');
    if (buildCode != OH_NN_SUCCESS) throw std::runtime_error("Public model Build failed");
    handles.executor = api.OH_NNExecutor_Construct(handles.compilation);
    if (!handles.executor) throw std::runtime_error("Public executor construction failed");
    size_t inputCount = 0, outputCount = 0;
    const auto inputCode = api.OH_NNExecutor_GetInputCount(handles.executor, &inputCount);
    const auto outputCode = api.OH_NNExecutor_GetOutputCount(handles.executor, &outputCount);
    report.step("{\"stage\":\"tensor_counts\",\"inputCode\":" + std::to_string(int(inputCode)) +
                ",\"inputCount\":" + std::to_string(inputCount) + ",\"outputCode\":" +
                std::to_string(int(outputCode)) + ",\"outputCount\":" + std::to_string(outputCount) + '}');
    if (inputCode != OH_NN_SUCCESS || outputCode != OH_NN_SUCCESS || inputCount != 2 || outputCount != 2)
        throw std::runtime_error("The audited model requires exactly two inputs and two outputs");
    std::array<void*, 2> inputMemory{}, outputMemory{};
    for (size_t i = 0; i < 2; ++i) inputMemory[i] = AllocateTensor(api, handles, deviceId, true, i, report);
    for (size_t i = 0; i < 2; ++i) outputMemory[i] = AllocateTensor(api, handles, deviceId, false, i, report);
    for (bool planar : {true, false}) {
        for (bool identity : {true, false}) {
            report.cases.push_back(RunCase(api, handles, inputMemory, outputMemory, planar, identity, report));
            report.event("{\"stage\":\"case_complete\",\"completedCaseCount\":" + std::to_string(report.cases.size()) + '}');
        }
    }
    report.event("{\"stage\":\"all_cases_returned_cleanup_follows\"}");
    return "offscreen_diagnostic_completed";
}
} // namespace

std::string inspectNpuFlowInterpolation(const std::string& filesDir)
{
    static std::mutex benchmarkMutex;
    std::lock_guard<std::mutex> lock(benchmarkMutex);
    Report report;
    try {
        const auto status = Execute(filesDir, report); // RAII cleanup finishes before returning here.
        return report.finish(status);
    } catch (const std::exception& exception) {
        return report.finish("diagnostic_failed", exception.what());
    } catch (...) {
        return report.finish("diagnostic_failed", "Unknown native exception");
    }
}
} // namespace muguet::fg_probe
