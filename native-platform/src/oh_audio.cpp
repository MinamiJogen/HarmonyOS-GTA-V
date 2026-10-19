#include "muguet_platform.h"

#include <ohaudio/native_audiostreambuilder.h>
#include <ohaudio/native_audiorenderer.h>

#include <atomic>
#include <cstring>
#include <memory>
#include <mutex>

namespace {
struct AudioOutput {
    OH_AudioRenderer *renderer = nullptr;
    MgAudioCallback callback = nullptr;
    void *user = nullptr;
    std::atomic<bool> paused{false};
    ~AudioOutput()
    {
        if (renderer != nullptr) {
            OH_AudioRenderer_Stop(renderer);
            OH_AudioRenderer_Release(renderer);
        }
    }
};

std::mutex outputMutex;
std::unique_ptr<AudioOutput> output;

OH_AudioData_Callback_Result WriteData(OH_AudioRenderer *, void *user, void *data, int32_t bytes)
{
    auto *state = static_cast<AudioOutput *>(user);
    if (data == nullptr || bytes <= 0) return AUDIO_DATA_CALLBACK_RESULT_INVALID;
    std::memset(data, 0, static_cast<size_t>(bytes));
    if (!state->paused.load(std::memory_order_relaxed)) {
        const auto frames = static_cast<uint32_t>(bytes) / (2 * sizeof(float));
        state->callback(state->user, static_cast<float *>(data), frames);
    }
    return AUDIO_DATA_CALLBACK_RESULT_VALID;
}

OH_AudioStream_Result CreateRenderer(AudioOutput &state, OH_AudioStream_LatencyMode mode)
{
    OH_AudioStreamBuilder *builder = nullptr;
    auto result = OH_AudioStreamBuilder_Create(&builder, AUDIOSTREAM_TYPE_RENDERER);
    if (result != AUDIOSTREAM_SUCCESS) return result;
    auto set = [&](OH_AudioStream_Result next) {
        if (result == AUDIOSTREAM_SUCCESS) result = next;
    };
    set(OH_AudioStreamBuilder_SetSamplingRate(builder, 48000));
    set(OH_AudioStreamBuilder_SetChannelCount(builder, 2));
    set(OH_AudioStreamBuilder_SetSampleFormat(builder, AUDIOSTREAM_SAMPLE_F32LE));
    set(OH_AudioStreamBuilder_SetEncodingType(builder, AUDIOSTREAM_ENCODING_TYPE_RAW));
    set(OH_AudioStreamBuilder_SetLatencyMode(builder, mode));
    set(OH_AudioStreamBuilder_SetRendererInfo(builder, AUDIOSTREAM_USAGE_GAME));
    set(OH_AudioStreamBuilder_SetRendererWriteDataCallback(builder, WriteData, &state));
    if (result == AUDIOSTREAM_SUCCESS) {
        result = OH_AudioStreamBuilder_GenerateRenderer(builder, &state.renderer);
    }
    OH_AudioStreamBuilder_Destroy(builder);
    return result;
}
} // namespace

extern "C" int32_t mg_audio_start(MgAudioCallback callback, void *user)
{
    if (callback == nullptr) return AUDIOSTREAM_ERROR_INVALID_PARAM;
    std::lock_guard<std::mutex> lock(outputMutex);
    if (output) return AUDIOSTREAM_ERROR_ILLEGAL_STATE;
    auto state = std::make_unique<AudioOutput>();
    state->callback = callback;
    state->user = user;
    auto result = CreateRenderer(*state, AUDIOSTREAM_LATENCY_MODE_FAST);
    if (result != AUDIOSTREAM_SUCCESS) {
        // Some routes do not offer fast float PCM. Retry the same real
        // renderer in normal mode without changing the game's sample format.
        if (state->renderer != nullptr) {
            OH_AudioRenderer_Release(state->renderer);
            state->renderer = nullptr;
        }
        result = CreateRenderer(*state, AUDIOSTREAM_LATENCY_MODE_NORMAL);
    }
    if (result != AUDIOSTREAM_SUCCESS) return result;
    result = OH_AudioRenderer_Start(state->renderer);
    if (result != AUDIOSTREAM_SUCCESS) return result;
    output = std::move(state);
    return AUDIOSTREAM_SUCCESS;
}

extern "C" void mg_audio_pause(int32_t paused)
{
    std::lock_guard<std::mutex> lock(outputMutex);
    if (output) output->paused.store(paused != 0, std::memory_order_relaxed);
}

extern "C" void mg_audio_stop(void)
{
    std::lock_guard<std::mutex> lock(outputMutex);
    output.reset();
}
