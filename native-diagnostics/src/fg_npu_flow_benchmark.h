#ifndef FG_NPU_FLOW_BENCHMARK_H
#define FG_NPU_FLOW_BENCHMARK_H

#include <string>

namespace muguet::fg_probe {

// Opt-in, synchronous offscreen diagnostic. Run on a worker in a disposable
// diagnostic process. Uses only public NNRT/CANN APIs and writes generated
// texture/GT/CPU-warp PPMs plus JSON diagnostics below filesDir. Stock model
// bytes are read with a strict <8 MiB bound, transformed only in memory, and
// erased after executor/compilation cleanup. No model bytes are exported.
// Successful calls establish neither presentation FPS nor actual NPU placement.
std::string inspectNpuFlowInterpolation(const std::string& filesDir);

} // namespace muguet::fg_probe
#endif
