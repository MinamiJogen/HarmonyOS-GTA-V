#ifndef FG_MODEL_METADATA_AUDIT_H
#define FG_MODEL_METADATA_AUDIT_H

#include <string>

namespace muguet::fg_probe {

// Synchronous, opt-in diagnostic: record NNRT devices before/after loading the
// public CANN library, then inspect stock aifi1.om/aifi2.om as raw files and as
// in-memory ROR1 buffers matching the observed Core transform. Each stock file
// must be regular, nonempty and strictly smaller than 8 MiB. Transformed buffers
// are compatibility-checked before compilation and erased before return.
// Call on a worker thread before a separate FG trial. No inference, tensor data
// allocation, model persistence/export, cache configuration or cwd changes.
// A successful metadata query does not establish FG compatibility or semantics.
std::string inspectSystemModelMetadata();

} // namespace muguet::fg_probe

#endif
