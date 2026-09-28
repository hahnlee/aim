#pragma once

#include <condition_variable>
#include <cstdint>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <thread>
#include <vector>

namespace aim::classloader_native {

using LoaderId = uint64_t;
using NamespaceId = uint64_t;
using Handle = uint64_t;

constexpr LoaderId kBootLoader = 0;
constexpr NamespaceId kBootNamespace = 0;

enum class OnLoadResult { kAbsent, kOkay, kJniError, kBadVersion };
enum class CollectResult { kIgnoredBoot, kDeferred, kUnloaded };

struct Owner {
  LoaderId loader;
  NamespaceId name_space;
  std::string path;
};

class Backend {
public:
  virtual ~Backend() = default;
  virtual void NewNamespaceWeakGlobal(LoaderId loader) = 0;
  virtual Handle Open(NamespaceId name_space, const std::string &path) = 0;
  virtual void NewLibraryWeakGlobal(LoaderId loader) = 0;
  virtual OnLoadResult OnLoad(Handle handle) = 0;
  virtual void OnUnload(Handle handle) = 0;
  virtual void DeleteLibraryWeakGlobal(LoaderId loader) = 0;
  virtual void Close(Handle handle) = 0;
};

class Registry {
public:
  explicit Registry(Backend &backend);

  bool Load(const std::string &path, LoaderId loader, std::string *error);
  CollectResult Collect(LoaderId loader);
  void Shutdown();

  std::optional<Owner> OwnerOf(Handle handle) const;
  bool RegisterFunctionPointer(Handle handle, uintptr_t address);
  std::optional<Owner> OwnerOfFunctionPointer(uintptr_t address) const;
  std::optional<Handle> Resolve(const std::string &path, LoaderId loader) const;
  std::optional<NamespaceId> NamespaceFor(LoaderId loader) const;
  size_t ResidentCount() const;
  size_t NamespaceCount() const;

private:
  enum class State {
    kOpening,
    kOnLoad,
    kOkay,
    kFailed,
    kOpenFailed,
    kUnloading,
    kClosed
  };

  struct Entry {
    Entry(std::string path_value, LoaderId loader_value,
          NamespaceId namespace_value)
        : path(std::move(path_value)), loader(loader_value),
          name_space(namespace_value) {}

    std::string path;
    LoaderId loader;
    NamespaceId name_space;
    Handle handle = 0;
    State state = State::kOpening;
    std::thread::id initializing_thread;
    std::string error;
    std::vector<uintptr_t> function_pointers;
    std::condition_variable changed;
  };

  NamespaceId NamespaceForLocked(LoaderId loader);
  void Teardown(const std::vector<std::shared_ptr<Entry>> &entries);

  Backend &backend_;
  mutable std::mutex mutex_;
  std::map<std::string, std::shared_ptr<Entry>> libraries_;
  std::map<Handle, std::weak_ptr<Entry>> owners_;
  std::map<uintptr_t, std::weak_ptr<Entry>> function_owners_;
  std::map<LoaderId, NamespaceId> namespaces_;
  NamespaceId next_namespace_ = 1;
};

} // namespace aim::classloader_native
