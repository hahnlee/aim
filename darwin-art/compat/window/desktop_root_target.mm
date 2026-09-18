#import <AppKit/AppKit.h>
#import <CoreFoundation/CoreFoundation.h>

#include "desktop_root_target.h"

#include <dispatch/dispatch.h>

#include <new>
#include <unordered_map>
#include <vector>

namespace darwin_art::window {
namespace {

struct TargetCatalog final {
  std::mutex mutex;
  std::unordered_map<uint64_t, std::weak_ptr<DesktopRootTarget>> entries;
};

TargetCatalog& Catalog() noexcept {
  static TargetCatalog catalog;
  return catalog;
}

bool MainThread() noexcept {
  return [NSThread isMainThread];
}

void ReleaseRetainedWindow(void* value) noexcept {
  if (value == nullptr) return;
  if (MainThread()) {
    CFRelease(static_cast<CFTypeRef>(value));
    return;
  }
  // The opaque +1 is intentionally kept as a CF ownership token.  The block
  // carries no Objective-C strong field, so the final NSWindow release cannot
  // occur on the worker that dropped the last DesktopRootTarget reference.
  dispatch_async(dispatch_get_main_queue(), ^{
    CFRelease(static_cast<CFTypeRef>(value));
  });
}

}  // namespace

DesktopRootTarget::DesktopRootTarget(std::shared_ptr<DesktopRootEvents> owner,
                                     NSWindow* window) noexcept
    : incarnation_(owner == nullptr ? 0 : owner->incarnation()),
      owner_(std::move(owner)),
      retained_window_(window == nil
                           ? nullptr
                           : const_cast<void*>(
                                 static_cast<const void*>(
                                     CFBridgingRetain(window)))) {}

std::shared_ptr<DesktopRootTarget> DesktopRootTarget::Create(
    std::shared_ptr<DesktopRootEvents> owner, NSWindow* window) noexcept {
  if (!MainThread() || owner == nullptr || window == nil ||
      owner->closed()) {
    return nullptr;
  }
  try {
    return std::shared_ptr<DesktopRootTarget>(
        new DesktopRootTarget(std::move(owner), window));
  } catch (const std::bad_alloc&) {
    return nullptr;
  }
}

bool DesktopRootTarget::Publish() noexcept {
  if (!MainThread()) return false;
  std::shared_ptr<DesktopRootTarget> self;
  try {
    self = shared_from_this();
  } catch (const std::bad_weak_ptr&) {
    return false;
  }
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (retired_ || published_ || incarnation_ == 0 || owner_ == nullptr ||
        retained_window_ == nullptr) {
      return false;
    }
  }
  try {
    auto& catalog = Catalog();
    std::lock_guard<std::mutex> lock(catalog.mutex);
    const auto inserted = catalog.entries.emplace(incarnation_, self);
    if (!inserted.second) return false;
  } catch (const std::bad_alloc&) {
    return false;
  }
  bool remove_publication = false;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (retired_ || published_) {
      // Publication cannot normally race AppKit-main-only lifecycle, but
      // retain the exact-entry removal rule if a future caller does.
      remove_publication = true;
    } else {
      published_ = true;
    }
  }
  if (remove_publication) RemovePublished(self);
  if (remove_publication) return false;
  return true;
}

void DesktopRootTarget::RemovePublished(
    const std::shared_ptr<DesktopRootTarget>& target) noexcept {
  if (target == nullptr) return;
  auto& catalog = Catalog();
  std::lock_guard<std::mutex> lock(catalog.mutex);
  const auto found = catalog.entries.find(target->incarnation_);
  if (found == catalog.entries.end()) return;
  const auto existing = found->second.lock();
  if (existing == nullptr || existing == target) {
    catalog.entries.erase(found);
  }
}

std::shared_ptr<DesktopRootTarget> AcquireProcessDesktopRootTarget() noexcept {
  std::vector<std::shared_ptr<DesktopRootTarget>> candidates;
  try {
    auto& catalog = Catalog();
    std::lock_guard<std::mutex> lock(catalog.mutex);
    for (auto it = catalog.entries.begin(); it != catalog.entries.end();) {
      auto target = it->second.lock();
      if (target == nullptr) {
        it = catalog.entries.erase(it);
        continue;
      }
      candidates.push_back(std::move(target));
      ++it;
    }
  } catch (const std::bad_alloc&) {
    return nullptr;
  }

  std::shared_ptr<DesktopRootTarget> only;
  for (auto& target : candidates) {
    if (target->IsRetired()) {
      DesktopRootTarget::RemovePublished(target);
      continue;
    }
    if (only != nullptr) return nullptr;
    only = std::move(target);
  }
  return only;
}

uint64_t DesktopRootTarget::incarnation() const noexcept {
  return incarnation_;
}

bool DesktopRootTarget::IsRetired() const noexcept {
  std::shared_ptr<DesktopRootEvents> owner;
  std::lock_guard<std::mutex> lock(mutex_);
  if (retired_) return true;
  owner = owner_;
  return owner != nullptr && owner->closed();
}

std::shared_ptr<DesktopRootEvents> DesktopRootTarget::RetainEvents() const noexcept {
  std::shared_ptr<DesktopRootEvents> owner;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (retired_) return nullptr;
    owner = owner_;
  }
  return owner != nullptr && !owner->closed() ? owner : nullptr;
}

bool DesktopRootTarget::Bind(
    const DarwinArtDesktopRootObserver& observer) noexcept {
  if (!MainThread() || observer.callback == nullptr) return false;
  std::shared_ptr<DesktopRootEvents> owner;
  NSWindow* window = nil;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (retired_ || retained_window_ == nullptr || owner_ == nullptr) {
      return false;
    }
    owner = owner_;
    window = (__bridge NSWindow*)retained_window_;
  }
  if (owner->closed()) {
    std::shared_ptr<DesktopRootTarget> self;
    try {
      self = shared_from_this();
    } catch (const std::bad_weak_ptr&) {
      return false;
    }
    {
      std::lock_guard<std::mutex> lock(mutex_);
      retired_ = true;
      published_ = false;
    }
    RemovePublished(self);
    return false;
  }
  return owner->Bind(window, observer.callback, observer.context);
}

bool DesktopRootTarget::Close() noexcept {
  if (!MainThread()) return false;
  std::shared_ptr<DesktopRootTarget> self;
  try {
    self = shared_from_this();
  } catch (const std::bad_weak_ptr&) {
    return false;
  }
  std::shared_ptr<DesktopRootEvents> owner;
  void* retained_window = nullptr;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (retired_) return false;
    retired_ = true;
    published_ = false;
    owner = std::move(owner_);
    retained_window = retained_window_;
    retained_window_ = nullptr;
  }
  // Remove the exact weak publication before dispatching any host callback.
  RemovePublished(self);
  if (owner != nullptr) (void)owner->Close();
  ReleaseRetainedWindow(retained_window);
  return true;
}

bool DesktopRootTarget::UnbindExpected(void* context) noexcept {
  if (!MainThread() || context == nullptr) return false;
  std::shared_ptr<DesktopRootEvents> owner;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    owner = owner_;
  }
  return owner != nullptr && owner->UnbindExpected(context);
}

bool DesktopRootTarget::Retire() noexcept { return Close(); }

DesktopRootTarget::~DesktopRootTarget() {
  std::shared_ptr<DesktopRootEvents> owner;
  void* retained_window = nullptr;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    retired_ = true;
    published_ = false;
    owner = std::move(owner_);
    retained_window = retained_window_;
    retained_window_ = nullptr;
  }
  if (MainThread()) {
    if (owner != nullptr) (void)owner->Close();
    ReleaseRetainedWindow(retained_window);
    return;
  }
  // A final background shared_ptr release must not release a live binding or
  // its original NSWindow on that worker.  Move both ownership tails to the
  // AppKit actor; no callback is run under the catalog lock.
  dispatch_async(dispatch_get_main_queue(), ^{
    if (owner != nullptr) (void)owner->Close();
    if (retained_window != nullptr) {
      CFRelease(static_cast<CFTypeRef>(retained_window));
    }
  });
}

}  // namespace darwin_art::window
