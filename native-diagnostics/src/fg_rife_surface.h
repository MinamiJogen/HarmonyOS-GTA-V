#pragma once
#include <atomic>
#include <cstdint>
#include <string>
#include <native_window/external_window.h>
namespace muguet::fg_probe {
// The caller retains the XComponent window until this bounded trial returns.
std::string runRifeSurface(OHNativeWindow* window,const std::string& directory,
    const std::string& fusionAwareMetadata,uint32_t seconds,std::atomic<bool>& stop);
}
