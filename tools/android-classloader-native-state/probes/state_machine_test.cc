#include "android_classloader_native_state.h"

#include <atomic>
#include <cassert>
#include <condition_variable>
#include <iostream>
#include <map>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

using aim::classloader_native::CollectResult;
using aim::classloader_native::Handle;
using aim::classloader_native::kBootLoader;
using aim::classloader_native::kBootNamespace;
using aim::classloader_native::LoaderId;
using aim::classloader_native::NamespaceId;
using aim::classloader_native::OnLoadResult;
using aim::classloader_native::Registry;

class FakeBackend final : public aim::classloader_native::Backend {
public:
  void NewNamespaceWeakGlobal(LoaderId loader) override {
    std::lock_guard lock(mutex_);
    events_.push_back("loader-" + std::to_string(loader) +
                      ":NewNamespaceWeakGlobalRef");
    ++namespace_weak_creates_[loader];
  }

  Handle Open(NamespaceId name_space, const std::string &path) override {
    std::lock_guard lock(mutex_);
    const Handle handle = next_handle_++;
    handles_[handle] = path;
    namespaces_[handle] = name_space;
    ++opens_[path];
    ++refs_[handle];
    events_.push_back(path + ":open");
    return handle;
  }

  void NewLibraryWeakGlobal(LoaderId loader) override {
    std::lock_guard lock(mutex_);
    events_.push_back("loader-" + std::to_string(loader) +
                      ":NewLibraryWeakGlobalRef");
    ++library_weak_creates_[loader];
  }

  OnLoadResult OnLoad(Handle handle) override {
    std::unique_lock lock(mutex_);
    const std::string path = handles_.at(handle);
    ++onloads_[path];
    events_.push_back(path + ":OnLoad");
    if (path == "libblocked.so") {
      blocked_onload_entered_ = true;
      changed_.notify_all();
      changed_.wait(lock, [&] { return release_blocked_onload_; });
    }
    if (path == "librecursive.so") {
      Registry *registry = registry_;
      lock.unlock();
      std::string error;
      const bool okay = registry->Load(path, 11, &error);
      assert(okay && error.empty());
      lock.lock();
    }
    if (path == "libfailed.so") {
      return OnLoadResult::kJniError;
    }
    if (path == "libabsent.so") {
      return OnLoadResult::kAbsent;
    }
    return OnLoadResult::kOkay;
  }

  void OnUnload(Handle handle) override {
    std::unique_lock lock(mutex_);
    const std::string path = handles_.at(handle);
    events_.push_back(path + ":OnUnload");
    if (path == "libteardown-race.so" && !teardown_released_) {
      teardown_entered_ = true;
      changed_.notify_all();
      changed_.wait(lock, [&] { return teardown_released_; });
    }
  }

  void DeleteLibraryWeakGlobal(LoaderId loader) override {
    std::lock_guard lock(mutex_);
    events_.push_back("loader-" + std::to_string(loader) +
                      ":DeleteWeakGlobalRef");
    ++weak_deletes_[loader];
  }

  void Close(Handle handle) override {
    std::lock_guard lock(mutex_);
    const std::string path = handles_.at(handle);
    events_.push_back(path + ":close");
    assert(refs_.at(handle) == 1);
    refs_[handle] = 0;
    ++closes_[path];
  }

  void Attach(Registry *registry) { registry_ = registry; }

  void WaitForBlockedOnLoad() {
    std::unique_lock lock(mutex_);
    changed_.wait(lock, [&] { return blocked_onload_entered_; });
  }

  void ReleaseBlockedOnLoad() {
    std::lock_guard lock(mutex_);
    release_blocked_onload_ = true;
    changed_.notify_all();
  }

  void WaitForTeardown() {
    std::unique_lock lock(mutex_);
    changed_.wait(lock, [&] { return teardown_entered_; });
  }

  void ReleaseTeardown() {
    std::lock_guard lock(mutex_);
    teardown_released_ = true;
    changed_.notify_all();
  }

  size_t Opens(const std::string &path) const {
    std::lock_guard lock(mutex_);
    return Value(opens_, path);
  }

  size_t Closes(const std::string &path) const {
    std::lock_guard lock(mutex_);
    return Value(closes_, path);
  }

  size_t OnLoads(const std::string &path) const {
    std::lock_guard lock(mutex_);
    return Value(onloads_, path);
  }

  size_t WeakDeletes(LoaderId loader) const {
    std::lock_guard lock(mutex_);
    auto found = weak_deletes_.find(loader);
    return found == weak_deletes_.end() ? 0 : found->second;
  }

  size_t LibraryWeakCreates(LoaderId loader) const {
    std::lock_guard lock(mutex_);
    auto found = library_weak_creates_.find(loader);
    return found == library_weak_creates_.end() ? 0 : found->second;
  }

  size_t NamespaceWeakCreates(LoaderId loader) const {
    std::lock_guard lock(mutex_);
    auto found = namespace_weak_creates_.find(loader);
    return found == namespace_weak_creates_.end() ? 0 : found->second;
  }

  NamespaceId NamespaceForHandle(Handle handle) const {
    std::lock_guard lock(mutex_);
    return namespaces_.at(handle);
  }

  std::vector<std::string> Events() const {
    std::lock_guard lock(mutex_);
    return events_;
  }

private:
  static size_t Value(const std::map<std::string, size_t> &values,
                      const std::string &key) {
    auto found = values.find(key);
    return found == values.end() ? 0 : found->second;
  }

  mutable std::mutex mutex_;
  std::condition_variable changed_;
  Registry *registry_ = nullptr;
  Handle next_handle_ = 0x1000;
  std::map<Handle, std::string> handles_;
  std::map<Handle, NamespaceId> namespaces_;
  std::map<Handle, size_t> refs_;
  std::map<std::string, size_t> opens_;
  std::map<std::string, size_t> closes_;
  std::map<std::string, size_t> onloads_;
  std::map<LoaderId, size_t> weak_deletes_;
  std::map<LoaderId, size_t> library_weak_creates_;
  std::map<LoaderId, size_t> namespace_weak_creates_;
  std::vector<std::string> events_;
  bool blocked_onload_entered_ = false;
  bool release_blocked_onload_ = false;
  bool teardown_entered_ = false;
  bool teardown_released_ = false;
};

static size_t EventIndex(const std::vector<std::string> &events,
                         const std::string &wanted, size_t start = 0) {
  for (size_t index = start; index < events.size(); ++index) {
    if (events[index] == wanted) {
      return index;
    }
  }
  assert(false && "expected lifecycle event missing");
  return events.size();
}

int main() {
  FakeBackend backend;
  Registry registry(backend);
  backend.Attach(&registry);
  std::string error;

  assert(registry.Load("libone.so", 11, &error));
  const Handle one = registry.Resolve("libone.so", 11).value();
  assert(registry.Load("libone.so", 11, &error));
  assert(backend.Opens("libone.so") == 1 && backend.OnLoads("libone.so") == 1);
  assert(!registry.Resolve("libone.so", 12).has_value());
  assert(!registry.Load("libone.so", 12, &error));
  assert(error.find("another ClassLoader") != std::string::npos);
  assert(registry.OwnerOf(one)->loader == 11);
  assert(registry.RegisterFunctionPointer(one, 0xfeed1000));
  assert(registry.RegisterFunctionPointer(one, 0xfeed1000));
  assert(registry.OwnerOfFunctionPointer(0xfeed1000)->path == "libone.so");
  assert(!registry.OwnerOfFunctionPointer(0xfeed2000).has_value());

  assert(registry.Load("libtwo.so", 11, &error));
  const Handle two = registry.Resolve("libtwo.so", 11).value();
  assert(!registry.RegisterFunctionPointer(two, 0xfeed1000));
  assert(registry.Load("libother.so", 12, &error));
  assert(registry.NamespaceFor(11) == registry.NamespaceFor(11));
  assert(registry.NamespaceFor(11) != registry.NamespaceFor(12));
  assert(registry.NamespaceCount() == 2);
  assert(backend.NamespaceWeakCreates(11) == 1 &&
         backend.NamespaceWeakCreates(12) == 1);

  assert(registry.Load("libboot.so", kBootLoader, &error));
  const Handle boot = registry.Resolve("libboot.so", kBootLoader).value();
  assert(backend.NamespaceForHandle(boot) == kBootNamespace);
  assert(registry.Collect(kBootLoader) == CollectResult::kIgnoredBoot);

  assert(registry.Load("librecursive.so", 11, &error));
  assert(backend.Opens("librecursive.so") == 1);

  assert(!registry.Load("libfailed.so", 13, &error));
  assert(!registry.Load("libfailed.so", 13, &error));
  assert(backend.Opens("libfailed.so") == 1 &&
         backend.OnLoads("libfailed.so") == 1);

  constexpr size_t kThreads = 24;
  std::atomic<size_t> successful{0};
  std::vector<std::thread> threads;
  for (size_t index = 0; index < kThreads; ++index) {
    threads.emplace_back([&] {
      std::string thread_error;
      if (registry.Load("libblocked.so", 14, &thread_error)) {
        ++successful;
      }
    });
  }
  backend.WaitForBlockedOnLoad();
  assert(registry.Collect(14) == CollectResult::kDeferred);
  backend.ReleaseBlockedOnLoad();
  for (std::thread &thread : threads) {
    thread.join();
  }
  assert(successful == kThreads);
  assert(backend.Opens("libblocked.so") == 1 &&
         backend.OnLoads("libblocked.so") == 1);

  assert(registry.Load("libteardown-race.so", 15, &error));
  std::thread collector(
      [&] { assert(registry.Collect(15) == CollectResult::kUnloaded); });
  backend.WaitForTeardown();
  std::atomic<bool> reload_done{false};
  std::thread reloader([&] {
    std::string reload_error;
    assert(registry.Load("libteardown-race.so", 15, &reload_error));
    reload_done = true;
  });
  std::this_thread::yield();
  assert(!reload_done.load());
  backend.ReleaseTeardown();
  collector.join();
  reloader.join();
  assert(backend.Opens("libteardown-race.so") == 2);
  assert(backend.Closes("libteardown-race.so") == 1);

  assert(registry.Collect(11) == CollectResult::kUnloaded);
  assert(!registry.OwnerOf(one).has_value());
  assert(!registry.OwnerOfFunctionPointer(0xfeed1000).has_value());
  assert(backend.LibraryWeakCreates(11) == 3);
  assert(backend.WeakDeletes(11) == 3);
  const std::vector<std::string> events = backend.Events();
  const size_t open = EventIndex(events, "libone.so:open");
  const size_t new_weak =
      EventIndex(events, "loader-11:NewLibraryWeakGlobalRef", open);
  const size_t onload = EventIndex(events, "libone.so:OnLoad", new_weak);
  const size_t unload = EventIndex(events, "libone.so:OnUnload");
  const size_t weak =
      EventIndex(events, "loader-11:DeleteWeakGlobalRef", unload);
  const size_t close = EventIndex(events, "libone.so:close", weak);
  assert(open < new_weak && new_weak < onload);
  assert(unload < weak && weak < close);

  const size_t namespaces_before_shutdown = registry.NamespaceCount();
  registry.Shutdown();
  assert(registry.ResidentCount() == 0);
  assert(registry.NamespaceCount() == namespaces_before_shutdown);
  assert(backend.LibraryWeakCreates(kBootLoader) == 0);
  assert(backend.WeakDeletes(kBootLoader) == 0);

  std::cout << "android-classloader-native-state: PASS same-path-cache=1 "
               "owner-refcount=1 "
               "cross-loader=reject namespaces=isolated boot=system\n";
  std::cout << "android-classloader-native-state: PASS concurrent-waiters="
            << kThreads << " recursive=success unload-during-OnLoad=deferred\n";
  std::cout << "android-classloader-native-state: PASS "
               "OnUnload<DeleteWeakGlobalRef<close "
               "failed-OnLoad=resident nativebridge-owner=exact\n";
  return 0;
}
