#!/usr/bin/env python3
"""Host-check the probe's actual rate policy and NAPI argument boundary."""
from pathlib import Path
import json
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]
CPP = ROOT / 'harmony/entry/src/main/cpp'
source = (CPP / 'graphics_frame_surface.cpp').read_text()
begin = source.index('napi_value configure(')
end = source.index('\nnapi_value status(', begin)
configure = source[begin:end]

PRELUDE = r'''
#include "graphics_frame_rate_policy.h"
#include <algorithm>
#include <cassert>
#include <cerrno>
#include <limits>
#include <mutex>
#include <string>
#include <sys/stat.h>
#include <unistd.h>
#include <vector>
struct Value {
    enum Type { Text, Bool, Number } type;
    std::string text;
    bool boolean = false;
    double number = 0;
};
using napi_env = void*;
using napi_callback_info = void*;
using napi_value = Value*;
constexpr int napi_ok = 0;
std::vector<napi_value> args;
Value result{Value::Bool, "", false, 0};
int napi_get_cb_info(napi_env, napi_callback_info, size_t* n,
                    napi_value* values, void*, void*) {
    *n = std::min(*n, args.size());
    for (size_t i = 0; i < *n; ++i) values[i] = args[i];
    return napi_ok;
}
int napi_get_value_bool(napi_env, napi_value value, bool* output) {
    if (!value || value->type != Value::Bool) return 1;
    *output = value->boolean; return napi_ok;
}
int napi_get_value_double(napi_env, napi_value value, double* output) {
    if (!value || value->type != Value::Number) return 1;
    *output = value->number; return napi_ok;
}
int napi_get_boolean(napi_env, bool value, napi_value* output) {
    result.boolean = value; *output = &result; return napi_ok;
}
std::string readString(napi_env, napi_value value) {
    return value && value->type == Value::Text ? value->text : "";
}
std::string quote(const std::string& text) { return "\"" + text + "\""; }
struct {
    std::mutex mutex;
    bool started = false, configured = false, enabled = true;
    bool uiPrediction = false, geometry = false, swapchainSampled = false, finalDepthState = false, rifeStream = false;
    uint32_t seconds = 20, sourceFps = 15, targetFps = 30;
    std::string directory, latest, fusionAwareMetadata;
    unsigned accepted = 0;
    void startLocked() { ++accepted; }
} engine;
'''

TEST = r'''
int main(int argc, char** argv) {
    assert(argc == 2);
    using namespace muguet::fg_probe;
    assert(kDefaultSourceFps == 15 && kDefaultTargetFps == 30);
    assert(validFrameRates(15, 30, true));
    assert(validFrameRates(30, 60, true));
    assert(validFrameRates(60, 144, true));
    assert(!validFrameRates(60, 60, true));
    assert(!validFrameRates(30, 30, true));
    assert(validFrameRates(60, 30, false));
    assert(validFrameRates(30, 30, false));
    for (double value : {14.0, 61.0, 15.5,
                        std::numeric_limits<double>::infinity(),
                        std::numeric_limits<double>::quiet_NaN()})
        assert(!validFrameRates(value, 144, true));
    for (double value : {29.0, 145.0, 30.5,
                        std::numeric_limits<double>::infinity(),
                        std::numeric_limits<double>::quiet_NaN()})
        assert(!validFrameRates(15, value, false));
    assert(sourceDeadlineNs(1, 15) == 66'666'666ULL);
    assert(sourceDeadlineNs(1, 30) == 33'333'333ULL);
    assert(sourceDeadlineNs(15, 15) == sourceDeadlineNs(30, 30));
    assert(sourceDeadlineNs(15, 15) == 1'000'000'000ULL);
    assert(sourceDeadlineNs(3600, 60) == 60'000'000'000ULL);
    // Compare equal-time samples at different source cadences, including wrap.
    for (unsigned second = 0; second < 60; ++second) {
        const double at15 = sourceDeadlineNs(second * 15, 15) / 1e9;
        const double at30 = sourceDeadlineNs(second * 30, 30) / 1e9;
        assert(movingBarX(at15, 320, 1106) == movingBarX(at30, 320, 1106));
        assert(geometryAngle(at15) == geometryAngle(at30));
    }
    assert(movingBarX(1, 0, 1106) == 360);
    assert(movingBarX(4, 0, 1106) == 334);
    assert(std::abs(geometryAngle(1) - .18f) < 1e-6f);
    Value root{Value::Text, argv[1]}, on{Value::Bool, "", true};
    Value off{Value::Bool, "", false}, seconds{Value::Number, "", false, 3};
    Value ui{Value::Bool, "", false}, geometry{Value::Bool, "", true};
    Value source{Value::Number, "", false, 15}, target{Value::Number, "", false, 30};
    Value metadata{Value::Text, "Vulkan"}, malformed{Value::Text, "15"};
    args = {&root, &on, &seconds};
    assert(configure(nullptr, nullptr)->boolean);
    assert(engine.sourceFps == 15 && engine.targetFps == 30);
    assert(!engine.uiPrediction && !engine.geometry);
    assert(engine.fusionAwareMetadata == "unavailable");
    args = {&root, &on, &seconds, &ui, &geometry, &source, &target, &metadata};
    assert(configure(nullptr, nullptr)->boolean);
    assert(engine.geometry && engine.fusionAwareMetadata == "Vulkan");
    source.number = 30; target.number = 60;
    assert(configure(nullptr, nullptr)->boolean);
    assert(engine.sourceFps == 30 && engine.targetFps == 60);
    auto reject = [&] {
        unsigned accepted = engine.accepted;
        uint32_t oldSource = engine.sourceFps, oldTarget = engine.targetFps;
        assert(!configure(nullptr, nullptr)->boolean);
        assert(engine.accepted == accepted);
        assert(engine.sourceFps == oldSource && engine.targetFps == oldTarget);
        assert(engine.latest.find("configuration_error") != std::string::npos);
    };
    target.number = 30; reject(); // Enabled source must be lower than target.
    args[1] = &off;
    assert(configure(nullptr, nullptr)->boolean); // Matching-source control.
    assert(!engine.enabled && engine.sourceFps == 30 && engine.targetFps == 30);
    source.number = 60;
    assert(configure(nullptr, nullptr)->boolean); // OFF need not interpolate.
    args[1] = &on; source.number = 15;
    args[5] = &malformed; reject(); args[5] = &source;
    source.number = 15.5; reject(); source.number = 15;
    target.number = 29; reject(); target.number = 145; reject(); target.number = 30;
    target.number = std::numeric_limits<double>::quiet_NaN(); reject(); target.number = 30;
    seconds.number = 3.5; reject(); seconds.number = 3;
    args[7] = &on; reject(); args[7] = &metadata;
    args.push_back(&metadata); reject(); args.pop_back();
    args.clear(); reject();
    args = {&root, &on, &seconds, &ui, &geometry, &source, &target, &metadata};
    engine.started = true;
    unsigned accepted = engine.accepted;
    assert(!configure(nullptr, nullptr)->boolean && engine.accepted == accepted);
}
'''

with tempfile.TemporaryDirectory(prefix='muguet-fg-rates-', dir='/private/tmp') as directory:
    temp = Path(directory)
    test = temp / 'actual-config.cpp'
    test.write_text(PRELUDE + configure + TEST)
    executable = temp / 'test-rates'
    subprocess.run(['clang++', '-std=c++17', '-Wall', '-Wextra', '-Werror',
                    '-Wno-missing-field-initializers', '-I', str(CPP), str(test),
                    '-o', str(executable)], check=True)
    subprocess.run([str(executable), str(temp)], check=True)
print(json.dumps({'rateBoundaries': 'pass', 'sourceDeadlineAndTimeBasedMotion': 'pass',
                  'actualNapiConfigure': 'pass', 'deviceOrDisplayFpsVerified': False}))
