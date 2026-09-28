#include "android_classloader_native_state.h"

#include <algorithm>
#include <thread>
#include <utility>

namespace aim::classloader_native {

Registry::Registry(Backend &backend) : backend_(backend) {}

NamespaceId Registry::NamespaceForLocked(LoaderId loader) {
  if (loader == kBootLoader) {
    return kBootNamespace;
  }
  auto [it, inserted] = namespaces_.try_emplace(loader, 0);
  if (inserted) {
    it->second = next_namespace_++;
    backend_.NewNamespaceWeakGlobal(loader);
  }
  return it->second;
}

bool Registry::Load(const std::string &path, LoaderId loader,
                    std::string *error) {
  if (error == nullptr) {
    return false;
  }
  error->clear();
  if (path.empty()) {
    *error = "native library path is empty";
    return false;
  }

  std::shared_ptr<Entry> entry;
  {
    std::unique_lock lock(mutex_);
    for (;;) {
      auto found = libraries_.find(path);
      if (found == libraries_.end()) {
        entry =
            std::make_shared<Entry>(path, loader, NamespaceForLocked(loader));
        entry->initializing_thread = std::this_thread::get_id();
        libraries_.emplace(path, entry);
        break;
      }
      entry = found->second;
      if (entry->loader != loader) {
        *error = "shared library already belongs to another ClassLoader";
        return false;
      }
      if ((entry->state == State::kOpening || entry->state == State::kOnLoad) &&
          entry->initializing_thread == std::this_thread::get_id()) {
        return true;
      }
      while (entry->state == State::kOpening ||
             entry->state == State::kOnLoad ||
             entry->state == State::kUnloading) {
        entry->changed.wait(lock);
      }
      if (entry->state == State::kOkay) {
        return true;
      }
      if (entry->state == State::kFailed) {
        *error = entry->error;
        return false;
      }
      if (entry->state == State::kOpenFailed ||
          entry->state == State::kClosed) {
        if (libraries_.find(path) != libraries_.end()) {
          continue;
        }
        entry =
            std::make_shared<Entry>(path, loader, NamespaceForLocked(loader));
        entry->initializing_thread = std::this_thread::get_id();
        libraries_.emplace(path, entry);
        break;
      }
    }
  }

  const Handle handle = backend_.Open(entry->name_space, path);
  if (handle == 0) {
    std::lock_guard lock(mutex_);
    entry->state = State::kOpenFailed;
    entry->error = "native library open failed";
    auto found = libraries_.find(path);
    if (found != libraries_.end() && found->second == entry) {
      libraries_.erase(found);
    }
    entry->changed.notify_all();
    *error = entry->error;
    return false;
  }
  if (loader != kBootLoader) {
    backend_.NewLibraryWeakGlobal(loader);
  }

  {
    std::lock_guard lock(mutex_);
    entry->handle = handle;
    entry->state = State::kOnLoad;
    owners_[handle] = entry;
  }

  const OnLoadResult result = backend_.OnLoad(handle);
  {
    std::lock_guard lock(mutex_);
    switch (result) {
    case OnLoadResult::kAbsent:
    case OnLoadResult::kOkay:
      entry->state = State::kOkay;
      break;
    case OnLoadResult::kJniError:
      entry->state = State::kFailed;
      entry->error = "JNI_ERR returned from JNI_OnLoad";
      break;
    case OnLoadResult::kBadVersion:
      entry->state = State::kFailed;
      entry->error = "bad JNI version returned from JNI_OnLoad";
      break;
    }
    entry->initializing_thread = {};
    entry->changed.notify_all();
    if (entry->state == State::kFailed) {
      *error = entry->error;
      return false;
    }
  }
  return true;
}

CollectResult Registry::Collect(LoaderId loader) {
  if (loader == kBootLoader) {
    return CollectResult::kIgnoredBoot;
  }
  std::vector<std::shared_ptr<Entry>> victims;
  {
    std::lock_guard lock(mutex_);
    for (const auto &[_, entry] : libraries_) {
      if (entry->loader == loader &&
          (entry->state == State::kOpening || entry->state == State::kOnLoad)) {
        return CollectResult::kDeferred;
      }
    }
    for (auto it = libraries_.begin(); it != libraries_.end();) {
      if (it->second->loader == loader) {
        it->second->state = State::kUnloading;
        victims.push_back(it->second);
        ++it;
      } else {
        ++it;
      }
    }
  }
  Teardown(victims);
  return CollectResult::kUnloaded;
}

void Registry::Teardown(const std::vector<std::shared_ptr<Entry>> &entries) {
  for (const std::shared_ptr<Entry> &entry : entries) {
    backend_.OnUnload(entry->handle);
    if (entry->loader != kBootLoader) {
      backend_.DeleteLibraryWeakGlobal(entry->loader);
    }
    {
      std::lock_guard lock(mutex_);
      for (uintptr_t address : entry->function_pointers) {
        function_owners_.erase(address);
      }
    }
    backend_.Close(entry->handle);

    std::lock_guard lock(mutex_);
    owners_.erase(entry->handle);
    auto found = libraries_.find(entry->path);
    if (found != libraries_.end() && found->second == entry) {
      libraries_.erase(found);
    }
    entry->state = State::kClosed;
    entry->changed.notify_all();
  }
}

void Registry::Shutdown() {
  std::vector<std::shared_ptr<Entry>> victims;
  {
    std::lock_guard lock(mutex_);
    for (const auto &[_, entry] : libraries_) {
      if (entry->state != State::kOpening && entry->state != State::kOnLoad &&
          entry->state != State::kUnloading) {
        entry->state = State::kUnloading;
        victims.push_back(entry);
      }
    }
  }
  Teardown(victims);
}

std::optional<Owner> Registry::OwnerOf(Handle handle) const {
  std::lock_guard lock(mutex_);
  auto found = owners_.find(handle);
  if (found == owners_.end()) {
    return std::nullopt;
  }
  std::shared_ptr<Entry> entry = found->second.lock();
  if (entry == nullptr || entry->state == State::kClosed) {
    return std::nullopt;
  }
  return Owner{entry->loader, entry->name_space, entry->path};
}

bool Registry::RegisterFunctionPointer(Handle handle, uintptr_t address) {
  if (address == 0) {
    return false;
  }
  std::lock_guard lock(mutex_);
  auto handle_owner = owners_.find(handle);
  if (handle_owner == owners_.end()) {
    return false;
  }
  std::shared_ptr<Entry> entry = handle_owner->second.lock();
  if (entry == nullptr || entry->state == State::kUnloading ||
      entry->state == State::kClosed) {
    return false;
  }
  auto existing = function_owners_.find(address);
  if (existing != function_owners_.end()) {
    return existing->second.lock() == entry;
  }
  function_owners_[address] = entry;
  entry->function_pointers.push_back(address);
  return true;
}

std::optional<Owner> Registry::OwnerOfFunctionPointer(uintptr_t address) const {
  std::lock_guard lock(mutex_);
  auto found = function_owners_.find(address);
  if (found == function_owners_.end()) {
    return std::nullopt;
  }
  std::shared_ptr<Entry> entry = found->second.lock();
  if (entry == nullptr || entry->state == State::kUnloading ||
      entry->state == State::kClosed) {
    return std::nullopt;
  }
  return Owner{entry->loader, entry->name_space, entry->path};
}

std::optional<Handle> Registry::Resolve(const std::string &path,
                                        LoaderId loader) const {
  std::lock_guard lock(mutex_);
  auto found = libraries_.find(path);
  if (found == libraries_.end() || found->second->loader != loader ||
      found->second->state != State::kOkay) {
    return std::nullopt;
  }
  return found->second->handle;
}

std::optional<NamespaceId> Registry::NamespaceFor(LoaderId loader) const {
  if (loader == kBootLoader) {
    return kBootNamespace;
  }
  std::lock_guard lock(mutex_);
  auto found = namespaces_.find(loader);
  return found == namespaces_.end() ? std::nullopt
                                    : std::optional(found->second);
}

size_t Registry::ResidentCount() const {
  std::lock_guard lock(mutex_);
  return libraries_.size();
}

size_t Registry::NamespaceCount() const {
  std::lock_guard lock(mutex_);
  return namespaces_.size();
}

} // namespace aim::classloader_native
