#include "channel_identity_catalog.h"
#include "channel_resources.h"

#include <list>
#include <mutex>
#include <utility>
#include <vector>

namespace darwin_art::input {
namespace {
struct IdentityEntry {
  jweak token = nullptr;
  std::weak_ptr<InputChannelResources> core;
  // Destructors never use JNI. The live-env operation/Clear owns deletion.
};
void DeleteToken(JNIEnv* env, const std::shared_ptr<IdentityEntry>& entry) {
  if (auto token = std::exchange(entry->token, nullptr))
    env->DeleteWeakGlobalRef(token);
}
}

struct InputChannelIdentityCatalog::Control {
  std::mutex mutex;
  std::list<std::shared_ptr<IdentityEntry>> entries;
  size_t active = 0;
  uint64_t revision = 0;
  bool closed = false;
  bool cleared = false;

  struct Operation {
    JNIEnv* env;
    std::shared_ptr<Control> control;
    Operation(JNIEnv* current_env, std::shared_ptr<Control> candidate) : env(current_env) {
      std::lock_guard<std::mutex> lock(candidate->mutex);
      if (!candidate->closed) {
        ++candidate->active;
        control = std::move(candidate);
      }
    }
    ~Operation() {
      if (control == nullptr) return;
      std::list<std::shared_ptr<IdentityEntry>> garbage;
      {
        std::lock_guard<std::mutex> lock(control->mutex);
        // All snapshots/candidates were destroyed before the last guard.
        // Splice without allocation, and keep admission active through JNI.
        if (control->active == 1) {
          for (auto it = control->entries.begin(); it != control->entries.end();) {
            auto current = it++;
            if ((*current)->core.expired()) {
              garbage.splice(garbage.end(), control->entries, current);
              ++control->revision;
            }
          }
        }
      }
      for (const auto& entry : garbage) DeleteToken(env, entry);
      {
        std::lock_guard<std::mutex> lock(control->mutex);
        --control->active;
      }
    }
  };

  auto Snapshot(uint64_t* version) {
    std::vector<std::shared_ptr<IdentityEntry>> result;
    {
      std::lock_guard<std::mutex> lock(mutex);
      result.reserve(entries.size());
      for (const auto& entry : entries) result.push_back(entry);
      *version = revision;
    }
    return result;
  }
};

InputChannelIdentityCatalog::InputChannelIdentityCatalog()
    : control_(std::make_shared<Control>()) {}
InputChannelIdentityCatalog::~InputChannelIdentityCatalog() = default;
InputChannelIdentityCatalog& GetChannelIdentityCatalog() {
  static InputChannelIdentityCatalog catalog;
  return catalog;
}

ChannelIdentityResult InputChannelIdentityCatalog::Find(JNIEnv* env, jobject token) {
  if (env == nullptr || token == nullptr)
    return {ChannelIdentityStatus::kJniFailure, {}};
  Control::Operation operation(env, control_);
  if (operation.control == nullptr) return {ChannelIdentityStatus::kClosed, {}};
  if (env->ExceptionCheck()) return {ChannelIdentityStatus::kJniFailure, {}};
  uint64_t version;
  const auto snapshot = operation.control->Snapshot(&version);
  for (const auto& entry : snapshot) {
    auto core = entry->core.lock();
    if (core != nullptr && env->IsSameObject(entry->token, token) == JNI_TRUE)
      return {ChannelIdentityStatus::kResolved, std::move(core)};
    if (env->ExceptionCheck()) return {ChannelIdentityStatus::kJniFailure, {}};
  }
  return {};
}

ChannelIdentityResult InputChannelIdentityCatalog::Bind(
    JNIEnv* env, jobject token, std::shared_ptr<InputChannelResources> core) {
  if (env == nullptr || token == nullptr || core == nullptr)
    return {ChannelIdentityStatus::kJniFailure, {}};
  Control::Operation operation(env, control_);
  if (operation.control == nullptr) return {ChannelIdentityStatus::kClosed, {}};
  if (env->ExceptionCheck()) return {ChannelIdentityStatus::kJniFailure, {}};
  auto candidate = std::make_shared<IdentityEntry>();
  struct CandidateCleanup {
    JNIEnv* env;
    std::shared_ptr<IdentityEntry> entry;
    bool published = false;
    ~CandidateCleanup() { if (!published) DeleteToken(env, entry); }
  } cleanup{env, candidate};
  candidate->token = env->NewWeakGlobalRef(token);
  if (candidate->token == nullptr || env->ExceptionCheck())
    return {ChannelIdentityStatus::kJniFailure, {}};
  candidate->core = core;
  for (;;) {
    uint64_t version;
    const auto snapshot = operation.control->Snapshot(&version);
    for (const auto& entry : snapshot) {
      auto existing = entry->core.lock();
      if (existing != nullptr && env->IsSameObject(entry->token, token) == JNI_TRUE) {
        if (existing->Name() != core->Name())
          return {ChannelIdentityStatus::kConflict, {}};
        return {ChannelIdentityStatus::kResolved, std::move(existing)};
      }
      if (env->ExceptionCheck()) return {ChannelIdentityStatus::kJniFailure, {}};
    }
    {
      std::lock_guard<std::mutex> lock(operation.control->mutex);
      if (operation.control->revision != version) continue;
      operation.control->entries.push_back(candidate);
      cleanup.published = true;
      ++operation.control->revision;
      return {ChannelIdentityStatus::kResolved, std::move(core)};
    }
  }
}

bool InputChannelIdentityCatalog::CloseAdmission() {
  std::lock_guard<std::mutex> lock(control_->mutex);
  control_->closed = true;
  return control_->active == 0;
}

bool InputChannelIdentityCatalog::Clear(JNIEnv* env) {
  if (env == nullptr) return false;
  const auto control = control_;  // JNI cleanup may destroy the catalog owner.
  std::list<std::shared_ptr<IdentityEntry>> retired;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (!control->closed || control->active != 0) return false;
    if (control->cleared) return true;
    ++control->active;  // Keep concurrent close/clear from claiming quiescence.
    retired.splice(retired.end(), control->entries);
    ++control->revision;
  }
  for (const auto& entry : retired) DeleteToken(env, entry);
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    --control->active;
    control->cleared = true;
  }
  return true;
}
}  // namespace darwin_art::input
