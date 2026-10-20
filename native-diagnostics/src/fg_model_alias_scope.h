#ifndef FG_MODEL_ALIAS_SCOPE_H
#define FG_MODEL_ALIAS_SCOPE_H

#include <memory>
#include <string>

namespace muguet::fg_probe {
// Opt-in diagnostic. cwd is process-global; retain until all FG work stops.
class ModelAliasScope {
public:
    explicit ModelAliasScope(std::string filesDir);
    ~ModelAliasScope() noexcept;
    ModelAliasScope(const ModelAliasScope&) = delete;
    ModelAliasScope& operator=(const ModelAliasScope&) = delete;
    ModelAliasScope(ModelAliasScope&&) = delete;
    ModelAliasScope& operator=(ModelAliasScope&&) = delete;

    // Consume fg-model-alias-request.txt: aifi1/aifi2, with optional LF.
    // Without a marker, leave cwd unchanged. Never copy/modify model bytes.
    std::string prepareIfRequested();
    // 0 on success, otherwise fchdir errno. Failed restoration retains the
    // original fd and lock for another explicit attempt before destruction.
    int restore() noexcept;
private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};
} // namespace muguet::fg_probe
#endif
