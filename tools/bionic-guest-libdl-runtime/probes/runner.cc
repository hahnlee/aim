#include "aim_dso_namespace.h"
#include "aim_elf_loader.h"

#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

#include <algorithm>
#include <atomic>
#include <cerrno>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <unordered_map>
#include <utility>
#include <vector>

namespace {

constexpr int kAndroidRtldNow = 2;
constexpr int kAndroidRtldLocal = 0;
constexpr size_t kMaxDsoBytes = 64U * 1024U * 1024U;
constexpr const char *kPlugin = "libguest_libdl_plugin.so";
constexpr const char *kBadPlugin = "libguest_libdl_bad.so";

void Check(bool condition, const char *message) {
  if (!condition) {
    std::fprintf(stderr, "bionic-guest-libdl-runtime: FAIL %s\n", message);
    std::abort();
  }
}

void SetError(char *output, size_t capacity, const std::string &message) {
  if (output == nullptr || capacity == 0)
    return;
  const size_t copied = std::min(capacity - 1, message.size());
  std::memcpy(output, message.data(), copied);
  output[copied] = '\0';
}

std::vector<uint8_t> ReadRegularAt(int directory, const char *name) {
  if (name == nullptr || std::strchr(name, '/') != nullptr || name[0] == '\0') {
    return {};
  }
  const int fd = openat(directory, name, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
  if (fd < 0)
    return {};
  struct stat status{};
  if (fstat(fd, &status) != 0 || !S_ISREG(status.st_mode) ||
      status.st_size <= 0 ||
      static_cast<uint64_t>(status.st_size) > kMaxDsoBytes) {
    (void)close(fd);
    return {};
  }
  std::vector<uint8_t> bytes(static_cast<size_t>(status.st_size));
  size_t offset = 0;
  while (offset < bytes.size()) {
    const ssize_t count =
        read(fd, bytes.data() + offset, bytes.size() - offset);
    if (count < 0 && errno == EINTR)
      continue;
    if (count <= 0) {
      bytes.clear();
      break;
    }
    offset += static_cast<size_t>(count);
  }
  (void)close(fd);
  return bytes;
}

class Manager;
std::atomic<Manager *> g_manager{nullptr};

class Manager {
public:
  explicit Manager(int directory) : directory_(directory) {}

  ~Manager() {
    std::lock_guard lock(mutex_);
    Check(live_ == 0, "manager destroyed with live guest handles");
  }

  struct Entry {
    enum class State { kLoading, kLive, kClosing, kClosed, kFailed };
    explicit Entry(std::string value) : name(std::move(value)) {}
    std::string name;
    State state = State::kLoading;
    AimElfGraphHandle *graph = nullptr;
    size_t references = 0;
    size_t active = 0;
    std::condition_variable changed;
  };

  static void *OpenThunk(void *context, const char *filename, int flags,
                         const AimAndroidDlExtInfo *extinfo, char *error,
                         size_t capacity) {
    return static_cast<Manager *>(context)->Open(filename, flags, extinfo,
                                                 error, capacity);
  }

  static void *LookupThunk(void *context, void *handle, const char *symbol,
                           const char *version, char *error, size_t capacity) {
    return static_cast<Manager *>(context)->Lookup(handle, symbol, version,
                                                   error, capacity);
  }

  static int CloseThunk(void *context, void *handle, char *error,
                        size_t capacity) {
    return static_cast<Manager *>(context)->Close(handle, error, capacity);
  }

  static AimElfResolveStatus
  ResolveThunk(void *context, const AimElfSymbolRequest *request,
               uintptr_t *address, AimElfErrorBuffer *error) {
    return static_cast<Manager *>(context)->Resolve(request, address, error);
  }

  void Record(int phase) {
    std::lock_guard lock(mutex_);
    phases_.push_back(phase);
  }

  void *Open(const char *filename, int flags,
             const AimAndroidDlExtInfo *extinfo, char *error,
             size_t capacity) {
    if (filename == nullptr ||
        (std::strcmp(filename, kPlugin) != 0 &&
         std::strcmp(filename, kBadPlugin) != 0) ||
        std::strchr(filename, '/') != nullptr) {
      SetError(error, capacity, "guest libdl SONAME denied");
      return nullptr;
    }
    if (flags != (kAndroidRtldNow | kAndroidRtldLocal)) {
      rejected_flags_.fetch_add(1, std::memory_order_relaxed);
      SetError(error, capacity, "guest libdl flags denied");
      return nullptr;
    }
    if (extinfo != nullptr) {
      rejected_extinfo_.fetch_add(1, std::memory_order_relaxed);
      SetError(error, capacity, "android_dlopen_ext info unsupported");
      return nullptr;
    }

    std::shared_ptr<Entry> entry;
    {
      std::unique_lock lock(mutex_);
      for (;;) {
        auto existing = names_.find(filename);
        if (existing == names_.end())
          break;
        entry = existing->second;
        if (entry->state == Entry::State::kLoading ||
            entry->state == Entry::State::kClosing) {
          entry->changed.wait(lock);
          continue;
        }
        if (entry->state == Entry::State::kLive) {
          ++entry->references;
          ++same_handle_hits_;
          return entry.get();
        }
        names_.erase(existing);
      }
      entry = std::make_shared<Entry>(filename);
      names_.emplace(entry->name, entry);
      tokens_.emplace(entry.get(), entry);
      retained_.push_back(entry);
    }

    std::vector<uint8_t> bytes = ReadRegularAt(directory_, filename);
    AimElfGraphHandle *graph = nullptr;
    std::string failure;
    if (bytes.empty()) {
      failure = "guest sibling is unavailable";
    } else {
      AimElfGraphSource source{filename, bytes.data(), bytes.size()};
      const char *providers[] = {"libguest_lifecycle.so"};
      AimElfLoadOptions options{AIM_ELF_ABI_VERSION, &ResolveThunk,
                                      this};
      char message[512]{};
      AimElfErrorBuffer buffer{message, sizeof(message), 0};
      const AimElfStatus status = aim_elf_graph_load(
          filename, &source, 1, providers, 1, &options, &graph, &buffer);
      if (status != AIM_ELF_OK) {
        failure =
            message[0] == '\0' ? aim_elf_status_name(status) : message;
      }
    }

    {
      std::lock_guard lock(mutex_);
      if (graph == nullptr) {
        entry->state = Entry::State::kFailed;
        names_.erase(entry->name);
        ++failed_loads_;
        entry->changed.notify_all();
        SetError(error, capacity, failure);
        return nullptr;
      }
      entry->graph = graph;
      entry->references = 1;
      entry->state = Entry::State::kLive;
      ++live_;
      entry->changed.notify_all();
      return entry.get();
    }
  }

  void *Lookup(void *handle, const char *symbol, const char *version,
               char *error, size_t capacity) {
    if (handle == nullptr || symbol == nullptr || version != nullptr) {
      SetError(error, capacity, "guest dlsym argument denied");
      return nullptr;
    }
    std::shared_ptr<Entry> entry;
    {
      std::unique_lock lock(mutex_);
      auto found = tokens_.find(handle);
      if (found == tokens_.end() ||
          found->second->state != Entry::State::kLive) {
        SetError(error, capacity, "stale guest libdl handle");
        return nullptr;
      }
      entry = found->second;
      ++entry->active;
      if (block_lookup_) {
        lookup_admitted_ = true;
        gate_.notify_all();
        gate_.wait(lock, [this] { return release_lookup_; });
      }
    }

    uintptr_t address = 0;
    char message[512]{};
    AimElfErrorBuffer buffer{message, sizeof(message), 0};
    const AimElfStatus status = aim_elf_graph_lookup_root(
        entry->graph, symbol, &address, &buffer);
    {
      std::lock_guard lock(mutex_);
      Check(entry->active != 0, "lookup lease underflow");
      --entry->active;
      entry->changed.notify_all();
    }
    if (status != AIM_ELF_OK || address == 0) {
      SetError(error, capacity, std::string("guest dlsym missing: ") + symbol);
      return nullptr;
    }
    return reinterpret_cast<void *>(address);
  }

  int Close(void *handle, char *error, size_t capacity) {
    std::shared_ptr<Entry> entry;
    {
      std::unique_lock lock(mutex_);
      auto found = tokens_.find(handle);
      if (found == tokens_.end() ||
          found->second->state != Entry::State::kLive ||
          found->second->references == 0) {
        SetError(error, capacity, "stale guest libdl close");
        return -1;
      }
      entry = found->second;
      if (--entry->references != 0)
        return 0;
      entry->state = Entry::State::kClosing;
      entry->changed.wait(lock, [&] { return entry->active == 0; });
    }

    char message[512]{};
    AimElfErrorBuffer buffer{message, sizeof(message), 0};
    const AimElfStatus status =
        aim_elf_graph_unload(&entry->graph, &buffer);
    {
      std::lock_guard lock(mutex_);
      entry->state = status == AIM_ELF_OK ? Entry::State::kClosed
                                                 : Entry::State::kFailed;
      names_.erase(entry->name);
      Check(live_ != 0, "live handle underflow");
      --live_;
      entry->changed.notify_all();
    }
    if (status != AIM_ELF_OK) {
      SetError(error, capacity,
               message[0] == '\0' ? aim_elf_status_name(status)
                                  : message);
      return -1;
    }
    return 0;
  }

  AimElfResolveStatus Resolve(const AimElfSymbolRequest *request,
                                    uintptr_t *address,
                                    AimElfErrorBuffer *) {
    if (request == nullptr || address == nullptr ||
        request->symbol == nullptr) {
      return AIM_ELF_RESOLVE_ERROR;
    }
    if (request->version_soname != nullptr) {
      AimDsoResolution resolution{};
      if (aim_dso_resolve(request->version_soname, request->symbol,
                                 request->version_name, &resolution) == 0 &&
          resolution.provider == 1 && resolution.address != 0) {
        *address = resolution.address;
        return AIM_ELF_RESOLVE_FOUND;
      }
      return AIM_ELF_RESOLVE_ERROR;
    }
    if (std::strcmp(request->symbol, "guest_libdl_record") == 0) {
      bool exact_provider = false;
      for (size_t index = 0; index < request->needed_library_count; ++index) {
        const char *needed = request->needed_libraries[index];
        exact_provider = exact_provider ||
                         (needed != nullptr &&
                          std::strcmp(needed, "libguest_lifecycle.so") == 0);
      }
      if (!exact_provider)
        return AIM_ELF_RESOLVE_ERROR;
      *address = reinterpret_cast<uintptr_t>(&RecordPhase);
      return AIM_ELF_RESOLVE_FOUND;
    }
    return AIM_ELF_RESOLVE_ERROR;
  }

  void EnableLookupGate() {
    std::lock_guard lock(mutex_);
    block_lookup_ = true;
    lookup_admitted_ = false;
    release_lookup_ = false;
  }

  void WaitLookupAdmitted() {
    std::unique_lock lock(mutex_);
    gate_.wait(lock, [this] { return lookup_admitted_; });
  }

  void ReleaseLookupGate() {
    std::lock_guard lock(mutex_);
    release_lookup_ = true;
    block_lookup_ = false;
    gate_.notify_all();
  }

  size_t Live() const {
    std::lock_guard lock(mutex_);
    return live_;
  }
  size_t SameHandleHits() const {
    std::lock_guard lock(mutex_);
    return same_handle_hits_;
  }
  size_t FailedLoads() const {
    std::lock_guard lock(mutex_);
    return failed_loads_;
  }
  std::vector<int> Phases() const {
    std::lock_guard lock(mutex_);
    return phases_;
  }
  size_t RejectedFlags() const {
    return rejected_flags_.load(std::memory_order_relaxed);
  }
  size_t RejectedExtinfo() const {
    return rejected_extinfo_.load(std::memory_order_relaxed);
  }

  static void RecordPhase(int phase) {
    Manager *manager = g_manager.load(std::memory_order_acquire);
    Check(manager != nullptr, "lifecycle callback without manager");
    manager->Record(phase);
  }

private:
  int directory_;
  mutable std::mutex mutex_;
  std::condition_variable gate_;
  std::unordered_map<std::string, std::shared_ptr<Entry>> names_;
  std::unordered_map<void *, std::shared_ptr<Entry>> tokens_;
  std::vector<std::shared_ptr<Entry>> retained_;
  std::vector<int> phases_;
  size_t live_ = 0;
  size_t same_handle_hits_ = 0;
  size_t failed_loads_ = 0;
  bool block_lookup_ = false;
  bool lookup_admitted_ = false;
  bool release_lookup_ = false;
  std::atomic<size_t> rejected_flags_{0};
  std::atomic<size_t> rejected_extinfo_{0};
};

void *ResolveRoot(Manager &manager, AimElfGraphHandle *graph,
                  const char *symbol) {
  uintptr_t address = 0;
  char message[512]{};
  AimElfErrorBuffer error{message, sizeof(message), 0};
  Check(aim_elf_graph_lookup_root(graph, symbol, &address, &error) ==
                AIM_ELF_OK &&
            address != 0,
        symbol);
  (void)manager;
  return reinterpret_cast<void *>(address);
}

AimElfGraphHandle *LoadRoot(Manager &manager, int directory) {
  std::vector<uint8_t> bytes =
      ReadRegularAt(directory, "libguest_libdl_root.so");
  Check(!bytes.empty(), "read root DSO");
  AimElfGraphSource source{"libguest_libdl_root.so", bytes.data(),
                                 bytes.size()};
  const char *providers[] = {"libdl.so"};
  AimElfLoadOptions options{AIM_ELF_ABI_VERSION,
                                  &Manager::ResolveThunk, &manager};
  AimElfGraphHandle *graph = nullptr;
  char message[512]{};
  AimElfErrorBuffer error{message, sizeof(message), 0};
  Check(aim_elf_graph_load("libguest_libdl_root.so", &source, 1,
                                  providers, 1, &options, &graph,
                                  &error) == AIM_ELF_OK &&
            graph != nullptr,
        "load root DSO");
  return graph;
}

} // namespace

int main(int argc, char **argv) {
  if (argc != 2)
    return 10;
  const int directory = open(argv[1], O_RDONLY | O_DIRECTORY | O_CLOEXEC);
  Check(directory >= 0, "open trusted DSO directory");
  Manager manager(directory);
  g_manager.store(&manager, std::memory_order_release);
  AimLoaderCallbacks callbacks{&manager, &Manager::OpenThunk,
                                     &Manager::LookupThunk,
                                     &Manager::CloseThunk};
  Check(aim_loader_bind(&callbacks) == 0, "bind guest libdl callbacks");

  AimElfGraphHandle *root = LoadRoot(manager, directory);
  using OnLoad = int (*)(void *, void *);
  using Native = int (*)(void *, void *, int);
  OnLoad on_load =
      reinterpret_cast<OnLoad>(ResolveRoot(manager, root, "JNI_OnLoad"));
  Native native = reinterpret_cast<Native>(
      ResolveRoot(manager, root,
                  "Java_dev_aim_probe_GuestLibdlFixture_nativePlugin"));
  Check(on_load(nullptr, nullptr) == 0x00010006,
        "JNI_OnLoad guest dlopen path");
  Check(native(nullptr, nullptr, 35) == 42,
        "named JNI native guest dlopen path");
  Check(manager.Live() == 0 && manager.SameHandleHits() == 2 &&
            manager.RejectedFlags() == 2 && manager.RejectedExtinfo() == 2,
        "root refcount and fail-closed policy");

  void *held = aim_bionic_dlopen(kPlugin, kAndroidRtldNow);
  Check(held != nullptr, "open plugin for lookup-close race");
  manager.EnableLookupGate();
  std::atomic<void *> lookup_result{nullptr};
  std::atomic<bool> close_complete{false};
  std::thread lookup([&] {
    lookup_result.store(aim_bionic_dlsym(held, "guest_plugin_value"),
                        std::memory_order_release);
  });
  manager.WaitLookupAdmitted();
  std::thread closer([&] {
    Check(aim_bionic_dlclose(held) == 0, "close after lookup lease");
    close_complete.store(true, std::memory_order_release);
  });
  std::this_thread::sleep_for(std::chrono::milliseconds(20));
  Check(!close_complete.load(std::memory_order_acquire),
        "close waited for admitted lookup");
  manager.ReleaseLookupGate();
  lookup.join();
  closer.join();
  Check(lookup_result.load(std::memory_order_acquire) != nullptr &&
            close_complete.load(std::memory_order_acquire),
        "lookup-close drain completed");

  void *errors = aim_bionic_dlopen(kPlugin, kAndroidRtldNow);
  Check(errors != nullptr, "open plugin for TLS errors");
  std::string first_error;
  std::string second_error;
  std::thread first([&] {
    Check(aim_bionic_dlsym(errors, "thread_missing_one") == nullptr,
          "thread one missing symbol");
    char *value = aim_bionic_dlerror();
    Check(value != nullptr, "thread one dlerror");
    first_error = value;
    Check(aim_bionic_dlerror() == nullptr, "thread one consumes error");
  });
  std::thread second([&] {
    Check(aim_bionic_dlsym(errors, "thread_missing_two") == nullptr,
          "thread two missing symbol");
    char *value = aim_bionic_dlerror();
    Check(value != nullptr, "thread two dlerror");
    second_error = value;
    Check(aim_bionic_dlerror() == nullptr, "thread two consumes error");
  });
  first.join();
  second.join();
  Check(first_error.find("thread_missing_one") != std::string::npos &&
            second_error.find("thread_missing_two") != std::string::npos,
        "thread-local errors did not cross");
  Check(aim_bionic_dlclose(errors) == 0, "close TLS error plugin");

  Check(aim_bionic_dlopen(kBadPlugin, kAndroidRtldNow) == nullptr,
        "bad plugin load rejected");
  Check(aim_bionic_dlerror() != nullptr &&
            aim_bionic_dlerror() == nullptr &&
            manager.FailedLoads() == 1 && manager.Live() == 0,
        "failed private load cleanup");

  const std::vector<int> phases = manager.Phases();
  Check(phases.size() == 8, "constructor/finalizer count");
  for (size_t index = 0; index < phases.size(); index += 2) {
    Check(phases[index] == 1 && phases[index + 1] == 4,
          "constructor/finalizer order");
  }

  char message[512]{};
  AimElfErrorBuffer error{message, sizeof(message), 0};
  Check(aim_elf_graph_unload(&root, &error) == AIM_ELF_OK &&
            root == nullptr,
        "unload root DSO");
  g_manager.store(nullptr, std::memory_order_release);
  (void)close(directory);
  std::fprintf(
      stderr,
      "bionic-guest-libdl-runtime: PASS Android-ELF=JNI_OnLoad+named-JNI "
      "dlopen+dlsym+dlclose refcount=same-handle ctor/fini=4 "
      "dlerror=TLS+consume lookup-close=drained extinfo/flags=closed "
      "dyld=no\n");
  return 0;
}
