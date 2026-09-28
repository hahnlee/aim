#include "aim_native_bridge_loader.h"

#include <cstring>
#include <map>
#include <string>
#include <utility>
#include <vector>

namespace aim::integration {

enum class Format { kMachO, kAndroidElf };
enum class OnLoadResult { kAbsent, kOkay, kJniErr, kBadVersion };
enum class State { kPending, kOkay, kFailed };

struct Library {
  std::string path;
  uintptr_t class_loader_key;
  AimLoaderNamespace name_space;
  AimLoaderHandle handle;
  bool needs_native_bridge;
  State state;
};

class StateMachine {
 public:
  explicit StateMachine(const AimElfLoaderV1* loader) : loader_(loader) {}

  bool Load(const std::string& path,
            uintptr_t class_loader_key,
            Format format,
            OnLoadResult on_load,
            std::string* error) {
    auto existing = libraries_.find(path);
    if (existing != libraries_.end()) {
      if (existing->second.class_loader_key != class_loader_key) {
        *error = "library already belongs to another ClassLoader";
        return false;
      }
      if (existing->second.state == State::kPending) return true;  // recursive load
      if (existing->second.state == State::kFailed) {
        *error = "JNI_OnLoad failed on a previous attempt";
        return false;
      }
      return true;
    }

    Library library{path, class_loader_key, 0, nullptr, format == Format::kAndroidElf,
                    State::kPending};
    char buffer[128] = {};
    if (library.needs_native_bridge) {
      auto namespace_it = namespaces_.find(class_loader_key);
      if (namespace_it == namespaces_.end()) {
        AimLoaderNamespace created = loader_->create_namespace(
            loader_->context, 0, "/app/lib", "/app", buffer, sizeof(buffer));
        if (created == 0) {
          *error = buffer;
          return false;
        }
        namespace_it = namespaces_.emplace(class_loader_key, created).first;
      }
      library.name_space = namespace_it->second;
      library.handle = loader_->open(loader_->context, library.name_space, path.c_str(), 2,
                                     buffer, sizeof(buffer));
    } else {
      // A sentinel models a dyld handle. It must never enter the ELF loader callbacks.
      library.handle = reinterpret_cast<void*>(next_dyld_handle_++);
      ++dyld_open_count_;
    }
    if (library.handle == nullptr) {
      *error = buffer[0] == '\0' ? "open failed" : buffer;
      return false;
    }

    auto [inserted, won] = libraries_.emplace(path, library);
    if (!won) return false;
    Library& stored = inserted->second;
    bool okay = true;
    if (stored.needs_native_bridge) {
      void* trampoline = loader_->get_trampoline(
          loader_->context, stored.handle, "JNI_OnLoad", nullptr, 0,
          AIM_JNI_CALL_REGULAR, buffer, sizeof(buffer));
      if (trampoline == nullptr) {
        if (on_load != OnLoadResult::kAbsent) {
          okay = false;
          *error = "JNI_OnLoad trampoline failed";
        }
      } else if (on_load == OnLoadResult::kAbsent) {
        okay = false;
        *error = "JNI_OnLoad presence model mismatch";
      } else if (on_load == OnLoadResult::kJniErr) {
        okay = false;
        *error = "JNI_ERR returned from JNI_OnLoad";
      } else if (on_load == OnLoadResult::kBadVersion) {
        okay = false;
        *error = "bad JNI version returned from JNI_OnLoad";
      }
    }
    stored.state = okay ? State::kOkay : State::kFailed;
    // ART deliberately keeps a failed-OnLoad handle resident. No close rollback is safe.
    return okay;
  }

  void* Find(uintptr_t class_loader_key,
             const char* short_name,
             const char* long_name,
             const char* shorty,
             AimJniCallType call_type) {
    for (auto& [_, library] : libraries_) {
      if (library.class_loader_key != class_loader_key || library.state != State::kOkay ||
          !library.needs_native_bridge) {
        continue;
      }
      char error[128] = {};
      uint32_t length = shorty == nullptr ? 0 : static_cast<uint32_t>(std::strlen(shorty));
      void* result = loader_->get_trampoline(loader_->context, library.handle, short_name,
                                              shorty, length, call_type, error, sizeof(error));
      if (result == nullptr) {
        result = loader_->get_trampoline(loader_->context, library.handle, long_name,
                                         shorty, length, call_type, error, sizeof(error));
      }
      if (result != nullptr) return result;
    }
    return nullptr;
  }

  void Shutdown() {
    for (auto& [_, library] : libraries_) {
      if (library.needs_native_bridge) {
        char error[128] = {};
        // ART queries/calls JNI_OnUnload before destruction; missing is allowed.
        loader_->get_trampoline(loader_->context, library.handle, "JNI_OnUnload", nullptr, 0,
                                AIM_JNI_CALL_REGULAR, error, sizeof(error));
        loader_->close(loader_->context, library.handle, error, sizeof(error));
      } else {
        ++dyld_close_count_;
      }
    }
    libraries_.clear();
  }

  size_t library_count() const { return libraries_.size(); }
  size_t dyld_open_count() const { return dyld_open_count_; }
  size_t dyld_close_count() const { return dyld_close_count_; }

 private:
  const AimElfLoaderV1* loader_;
  std::map<uintptr_t, AimLoaderNamespace> namespaces_;
  std::map<std::string, Library> libraries_;
  uintptr_t next_dyld_handle_ = 0x1000;
  size_t dyld_open_count_ = 0;
  size_t dyld_close_count_ = 0;
};

}  // namespace aim::integration
