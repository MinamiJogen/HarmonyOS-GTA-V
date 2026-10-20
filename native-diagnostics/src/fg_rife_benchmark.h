#ifndef FG_RIFE_BENCHMARK_H
#define FG_RIFE_BENCHMARK_H

#include <string>

namespace muguet::fg_probe {
// Synchronous opt-in worker-thread diagnostic. Uses only the optional SDK's
// C ABI, app-owned rife-v4.6 model and fixed synthetic RGB8 fixtures. Writes
// bounded PPM/JSON evidence under filesDir/rife-benchmark; never presents.
std::string inspectRifeInterpolation(const std::string& filesDir);
}

#endif
