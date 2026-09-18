#include "egl_window_surface_owner.h"

#include <atomic>
#include <new>
#include <stdexcept>
#include <utility>

namespace darwin_art::graphics {

struct EglWindowSurfaceOwner::OwnerControl {
  std::mutex mutex;
  EglWindowSurfaceOwner* owner = nullptr;
};

struct EglWindowSurfaceOwner::DisplayDrain {
  enum class Phase { kActive, kEnrolling, kDraining, kTerminating,
                     kSucceeded, kFailed };
  EglWindowDisplay display = nullptr;
  std::uint64_t generation = 0;
  EglWindowDisplayTerminateCallback terminate = nullptr;
  void* terminate_context = nullptr;
  mutable std::mutex mutex;
  std::size_t remaining = 0;
  Phase phase = Phase::kActive;
  bool initialized = false;
  bool initialization_reserved = false;

  void Add() {
    std::lock_guard<std::mutex> lock(mutex);
    ++remaining;
  }

  void Complete() {
    EglWindowDisplayTerminateCallback callback = nullptr;
    void* context = nullptr;
    {
      std::lock_guard<std::mutex> lock(mutex);
      if (remaining != 0) --remaining;
      if (remaining == 0 && phase == Phase::kDraining) {
        phase = Phase::kTerminating;
        callback = terminate;
        context = terminate_context;
      }
    }
    if (callback == nullptr) return;
    const bool ok = callback(display, context);
    std::lock_guard<std::mutex> lock(mutex);
    phase = ok ? Phase::kSucceeded : Phase::kFailed;
  }

  void Seal() {
    {
      std::lock_guard<std::mutex> lock(mutex);
      if (phase != Phase::kEnrolling) return;
      phase = Phase::kDraining;
    }
  }

  void TryTerminate() {
    EglWindowDisplayTerminateCallback callback = nullptr;
    void* context = nullptr;
    {
      std::lock_guard<std::mutex> lock(mutex);
      if (remaining == 0 && phase == Phase::kDraining) {
        phase = Phase::kTerminating;
        callback = terminate;
        context = terminate_context;
      }
    }
    if (callback == nullptr) return;
    const bool ok = callback(display, context);
    std::lock_guard<std::mutex> lock(mutex);
    phase = ok ? Phase::kSucceeded : Phase::kFailed;
  }
};

struct EglWindowSurfaceOwner::Entry {
  enum class CloseResult { kAlreadyClosing, kReady, kBusy };

  struct CleanupCompletion {
    std::mutex mutex;
    std::vector<std::shared_ptr<DisplayDrain>> drains;
    bool complete = false;

    bool Prepare() {
      std::lock_guard<std::mutex> lock(mutex);
      try {
        drains.reserve(1);
        return true;
      } catch (...) {
        return false;
      }
    }

    void Enroll(const std::shared_ptr<DisplayDrain>& drain) {
      if (drain == nullptr) return;
      std::lock_guard<std::mutex> lock(mutex);
      if (complete) return;
      for (const auto& existing : drains) {
        if (existing.get() == drain.get()) return;
      }
      drains.push_back(drain);
      drain->Add();
    }

    void Complete() {
      std::vector<std::shared_ptr<DisplayDrain>> pending;
      {
        std::lock_guard<std::mutex> lock(mutex);
        if (complete) return;
        complete = true;
        pending.swap(drains);
      }
      for (const auto& drain : pending) drain->Complete();
    }
  };

  std::shared_ptr<OwnerControl> control;
  EglWindowHandle surface = nullptr;
  EglWindowDisplay display = nullptr;
  EglWindowSurfaceSnapshot value;
  EglWindowSurfaceCleanupCallback cleanup = nullptr;
  void* cleanup_context = nullptr;
  std::mutex mutex;
  bool operation_active = false;
  bool closing = false;
  bool cleanup_started = false;
  std::shared_ptr<CleanupCompletion> completion;

  struct Work {
    EglWindowSurfaceCleanupCallback callback = nullptr;
    void* context = nullptr;
    EglWindowSurfaceCleanup cleanup;
    std::shared_ptr<CleanupCompletion> completion;
    bool ready = false;
  };

  void RunWork(Work work) {
    if (!work.ready) return;
    if (work.callback != nullptr) work.callback(work.cleanup, work.context);
    if (work.completion != nullptr) work.completion->Complete();
    if (control != nullptr) {
      std::lock_guard<std::mutex> lock(control->mutex);
      // Finalization performs no provider callback. Keep destruction excluded
      // through this access, using the same control -> registry lock order.
      if (control->owner != nullptr) control->owner->FinalizeEntry(this);
    }
  }

  Work ReleaseOperation() {
    std::lock_guard<std::mutex> lock(mutex);
    operation_active = false;
    return TakeCleanupLocked();
  }

  bool PrepareCompletion() {
    std::lock_guard<std::mutex> lock(mutex);
    try {
      EnsureCompletionLocked();
      return completion->Prepare();
    } catch (...) {
      return false;
    }
  }

  CloseResult Close() {
    std::lock_guard<std::mutex> lock(mutex);
    if (closing) return CloseResult::kAlreadyClosing;
    closing = true;
    EnsureCompletionLocked();
    return operation_active ? CloseResult::kBusy : CloseResult::kReady;
  }

  // Closing and drain enrollment are one entry-lock transaction.  This is
  // also used when cleanup has already started: the completion object remains
  // enrollable until its callback returns, so termination cannot pass it.
  CloseResult CloseAndEnroll(const std::shared_ptr<DisplayDrain>& drain) {
    std::lock_guard<std::mutex> lock(mutex);
    const bool was_closing = closing;
    closing = true;
    EnsureCompletionLocked();
    completion->Enroll(drain);
    if (was_closing) return CloseResult::kAlreadyClosing;
    return operation_active ? CloseResult::kBusy : CloseResult::kReady;
  }

  Work MaybeCleanup() {
    std::lock_guard<std::mutex> lock(mutex);
    return TakeCleanupLocked();
  }

 private:
  void EnsureCompletionLocked() {
    if (completion == nullptr) completion = std::make_shared<CleanupCompletion>();
  }

  Work TakeCleanupLocked() {
    Work work;
    if (!closing || operation_active || cleanup_started) return work;
    cleanup_started = true;
    EnsureCompletionLocked();
    work.callback = cleanup;
    work.context = cleanup_context;
    work.cleanup.display = display;
    work.cleanup.surface = surface;
    work.cleanup.resources = value;
    work.completion = completion;
    work.ready = true;
    return work;
  }
};

EglWindowSurfaceOwner::Lease::Lease(std::shared_ptr<Entry> entry)
    : entry_(std::move(entry)) {}

EglWindowSurfaceOwner::Lease::Lease(Lease&& other) noexcept
    : entry_(std::move(other.entry_)) {}

EglWindowSurfaceOwner::Lease& EglWindowSurfaceOwner::Lease::operator=(
    Lease&& other) noexcept {
  if (this == &other) return *this;
  if (entry_ != nullptr) entry_->RunWork(entry_->ReleaseOperation());
  entry_ = std::move(other.entry_);
  return *this;
}

EglWindowSurfaceOwner::Lease::~Lease() {
  if (entry_ != nullptr) entry_->RunWork(entry_->ReleaseOperation());
}

EglWindowSurfaceSnapshot EglWindowSurfaceOwner::Lease::Snapshot() const {
  if (entry_ == nullptr) return {};
  std::lock_guard<std::mutex> lock(entry_->mutex);
  return entry_->value;
}

void EglWindowSurfaceOwner::Lease::SetTextureAndFramebuffer(
    std::uint32_t texture, std::uint32_t framebuffer, bool target_bound,
    bool tex_image_bound) {
  if (entry_ == nullptr) return;
  std::lock_guard<std::mutex> lock(entry_->mutex);
  entry_->value.texture = texture;
  entry_->value.framebuffer = framebuffer;
  entry_->value.target_bound = target_bound;
  entry_->value.tex_image_bound = tex_image_bound;
}

void EglWindowSurfaceOwner::Lease::SetIosurfaceTarget(
    void* iosurface, void* target, std::uint32_t width, std::uint32_t height,
    bool owns_ref) {
  if (entry_ == nullptr) return;
  std::lock_guard<std::mutex> lock(entry_->mutex);
  entry_->value.iosurface = iosurface;
  entry_->value.iosurface_target = target;
  entry_->value.width = width;
  entry_->value.height = height;
  entry_->value.owns_iosurface_ref = owns_ref;
  entry_->value.texture = 0;
  entry_->value.framebuffer = 0;
  entry_->value.target_bound = false;
  entry_->value.tex_image_bound = false;
}

void EglWindowSurfaceOwner::Lease::SetNativeBuffer(void* buffer) {
  if (entry_ == nullptr) return;
  std::lock_guard<std::mutex> lock(entry_->mutex);
  entry_->value.native_buffer = buffer;
}

void EglWindowSurfaceOwner::Lease::SetDimensions(
    std::uint32_t render_width, std::uint32_t render_height,
    std::uint32_t width, std::uint32_t height) {
  if (entry_ == nullptr) return;
  std::lock_guard<std::mutex> lock(entry_->mutex);
  entry_->value.render_width = render_width;
  entry_->value.render_height = render_height;
  entry_->value.width = width;
  entry_->value.height = height;
}

EglWindowSurfaceOwner& EglWindowSurfaceOwner::Instance() {
  static EglWindowSurfaceOwner owner;
  return owner;
}

EglWindowSurfaceOwner::DisplayInitializationLease::DisplayInitializationLease(
    std::shared_ptr<OwnerControl> control, std::shared_ptr<DisplayDrain> drain,
    std::shared_ptr<DisplayDrain> previous, bool restore_active)
    : control_(std::move(control)),
      drain_(std::move(drain)),
      previous_(std::move(previous)),
      restore_active_(restore_active) {}

EglWindowSurfaceOwner::DisplayInitializationLease::DisplayInitializationLease(
    DisplayInitializationLease&& other) noexcept
    : control_(std::move(other.control_)),
      drain_(std::move(other.drain_)),
      previous_(std::move(other.previous_)),
      restore_active_(other.restore_active_),
      completed_(other.completed_) {
  other.completed_ = true;
}

EglWindowSurfaceOwner::DisplayInitializationLease&
EglWindowSurfaceOwner::DisplayInitializationLease::operator=(
    DisplayInitializationLease&& other) noexcept {
  if (this == &other) return *this;
  Complete(false);
  control_ = std::move(other.control_);
  drain_ = std::move(other.drain_);
  previous_ = std::move(other.previous_);
  restore_active_ = other.restore_active_;
  completed_ = other.completed_;
  other.completed_ = true;
  return *this;
}

EglWindowSurfaceOwner::DisplayInitializationLease::~DisplayInitializationLease() {
  Complete(false);
}

bool EglWindowSurfaceOwner::DisplayInitializationLease::Complete(
    bool backend_succeeded) noexcept {
  if (completed_ || drain_ == nullptr) return false;
  completed_ = true;
  if (control_ == nullptr) return false;
  std::lock_guard<std::mutex> control_lock(control_->mutex);
  if (control_->owner == nullptr) return false;
  return control_->owner->CompleteDisplayInitialization(this,
                                                         backend_succeeded);
}

EglWindowSurfaceOwner::EglWindowSurfaceOwner()
    : control_(std::make_shared<OwnerControl>()) {
  control_->owner = this;
}

EglWindowSurfaceOwner::~EglWindowSurfaceOwner() {
  decltype(entries_) entries;
  {
    std::lock_guard<std::mutex> control_lock(control_->mutex);
    control_->owner = nullptr;
  }
  {
    std::lock_guard<std::mutex> lock(mutex_);
    entries.swap(entries_);
  }
  // Close and detach ownership while the instance is still structurally
  // alive, then invoke callbacks outside all owner locks. Active leases retain
  // their completion and finish safely after this destructor returns.
  for (const auto& [surface, entry] : entries) {
    (void)surface;
    (void)entry->Close();
    entry->RunWork(entry->MaybeCleanup());
  }
}

EglWindowSurfaceOwner::DisplayInitializationLease
EglWindowSurfaceOwner::ReserveDisplayInitialization(EglWindowDisplay display) {
  if (display == nullptr) return {};
  std::lock_guard<std::mutex> lock(mutex_);
  auto found = displays_.find(display);
  if (found != displays_.end()) {
    auto current = found->second;
    std::lock_guard<std::mutex> drain_lock(current->mutex);
    if (current->phase == DisplayDrain::Phase::kActive) {
      if (current->initialization_reserved) return {};
      current->initialization_reserved = true;
      return DisplayInitializationLease(control_, current, nullptr,
                                        current->initialized);
    }
    if (current->phase != DisplayDrain::Phase::kSucceeded &&
        current->phase != DisplayDrain::Phase::kFailed) return {};
    std::shared_ptr<DisplayDrain> prospective;
    try {
      prospective = std::make_shared<DisplayDrain>();
    } catch (...) {
      return {};
    }
    prospective->display = display;
    prospective->generation = next_generation_++;
    prospective->initialization_reserved = true;
    found->second = prospective;
    return DisplayInitializationLease(control_, std::move(prospective),
                                      std::move(current), false);
  }
  std::shared_ptr<DisplayDrain> prospective;
  try {
    prospective = std::make_shared<DisplayDrain>();
  } catch (...) {
    return {};
  }
  prospective->display = display;
  prospective->generation = next_generation_++;
  prospective->initialization_reserved = true;
  try {
    displays_.emplace(display, prospective);
  } catch (...) {
    return {};
  }
  return DisplayInitializationLease(control_, std::move(prospective), nullptr,
                                    false);
}

bool EglWindowSurfaceOwner::BeginDisplay(EglWindowDisplay display) {
  auto lease = ReserveDisplayInitialization(display);
  return lease.Complete(true);
}

bool EglWindowSurfaceOwner::IsDisplayInitialized(
    EglWindowDisplay display) const {
  if (display == nullptr) return false;
  std::shared_ptr<DisplayDrain> drain;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    auto found = displays_.find(display);
    if (found == displays_.end()) return false;
    drain = found->second;
  }
  std::lock_guard<std::mutex> lock(drain->mutex);
  return drain->phase == DisplayDrain::Phase::kActive &&
         drain->initialized && !drain->initialization_reserved;
}

bool EglWindowSurfaceOwner::CompleteDisplayInitialization(
    DisplayInitializationLease* lease, bool backend_succeeded) noexcept {
  if (lease == nullptr || lease->drain_ == nullptr) return false;
  std::lock_guard<std::mutex> lock(mutex_);
  auto found = displays_.find(lease->drain_->display);
  if (found == displays_.end() || found->second.get() != lease->drain_.get()) {
    return false;
  }
  auto current = found->second;
  std::lock_guard<std::mutex> drain_lock(current->mutex);
  if (!current->initialization_reserved ||
      current->phase != DisplayDrain::Phase::kActive) {
    return false;
  }
  current->initialization_reserved = false;
  if (backend_succeeded) {
    current->initialized = true;
    return true;
  }
  if (lease->previous_ != nullptr) {
    found->second = lease->previous_;
  } else if (!lease->restore_active_) {
    displays_.erase(found);
  }
  return false;
}

bool EglWindowSurfaceOwner::Create(
    EglWindowDisplay display, EglWindowHandle surface,
    EglWindowSurfaceCreateInfo resources,
    EglWindowSurfaceCleanupCallback cleanup, void* cleanup_context,
    bool* resources_cleaned) {
  if (resources_cleaned != nullptr) *resources_cleaned = false;
  if (display == nullptr || surface == nullptr || cleanup == nullptr) return false;
  EglWindowSurfaceCleanup rollback;
  rollback.display = display;
  rollback.surface = surface;
  rollback.resources = EglWindowSurfaceSnapshot{
      resources.host, resources.native_window, resources.native_buffer,
      resources.config, resources.bind_target, resources.iosurface,
      resources.iosurface_target, resources.texture_target, resources.texture,
      resources.framebuffer, resources.render_width, resources.render_height,
      resources.width, resources.height, resources.target_bound,
      resources.tex_image_bound, resources.owns_iosurface_ref};
  std::unique_lock<std::mutex> lock(mutex_);
  auto display_found = displays_.find(display);
  if (display_found != displays_.end()) {
    std::lock_guard<std::mutex> drain_lock(display_found->second->mutex);
    if (display_found->second->phase != DisplayDrain::Phase::kActive)
      return false;
    if (!display_found->second->initialized ||
        display_found->second->initialization_reserved)
      return false;
  } else {
    // A surface cannot be published before a successful display initialize.
    return false;
  }
  if (entries_.find(surface) != entries_.end()) return false;
  try {
    auto entry = std::make_shared<Entry>();
    entry->completion = std::make_shared<Entry::CleanupCompletion>();
    if (!entry->completion->Prepare()) throw std::bad_alloc();
    entry->control = control_;
    entry->surface = surface;
    entry->display = display;
    entry->value = rollback.resources;
    entry->cleanup = cleanup;
    entry->cleanup_context = cleanup_context;
    if (!entry->PrepareCompletion()) {
      lock.unlock();
      cleanup(rollback, cleanup_context);
      if (resources_cleaned != nullptr) *resources_cleaned = true;
      return false;
    }
    entries_.emplace(surface, std::move(entry));
  } catch (...) {
    lock.unlock();
    cleanup(rollback, cleanup_context);
    if (resources_cleaned != nullptr) *resources_cleaned = true;
    return false;
  }
  return true;
}

EglWindowSurfaceOwner::Lease EglWindowSurfaceOwner::Acquire(
    EglWindowDisplay display, EglWindowHandle surface,
    EglWindowSurfaceAdmission* admission) {
  if (admission != nullptr) *admission = EglWindowSurfaceAdmission::kUnknown;
  if (display == nullptr || surface == nullptr) return {};
  std::shared_ptr<Entry> entry;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    auto found = entries_.find(surface);
    if (found == entries_.end()) return {};
    entry = found->second;
    if (entry->display != display) {
      if (admission != nullptr)
        *admission = EglWindowSurfaceAdmission::kWrongDisplay;
      return {};
    }
  }
  {
    std::lock_guard<std::mutex> lock(entry->mutex);
    if (entry->closing) {
      if (admission != nullptr)
        *admission = EglWindowSurfaceAdmission::kClosing;
      return {};
    }
    if (entry->operation_active) {
      if (admission != nullptr)
        *admission = EglWindowSurfaceAdmission::kBusy;
      return {};
    }
    entry->operation_active = true;
  }
  if (admission != nullptr) *admission = EglWindowSurfaceAdmission::kAdmitted;
  return Lease(std::move(entry));
}

EglWindowSurfaceOwner::Lease EglWindowSurfaceOwner::Swap(
    EglWindowDisplay display, EglWindowHandle surface,
    EglWindowSurfaceAdmission* admission) {
  return Acquire(display, surface, admission);
}

bool EglWindowSurfaceOwner::Destroy(EglWindowDisplay display,
                                    EglWindowHandle surface,
                                    EglWindowSurfaceAdmission* admission) {
  if (admission != nullptr) *admission = EglWindowSurfaceAdmission::kUnknown;
  if (display == nullptr || surface == nullptr) return false;
  std::shared_ptr<Entry> entry;
  std::shared_ptr<DisplayDrain> drain;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    auto found = entries_.find(surface);
    if (found == entries_.end()) return false;
    entry = found->second;
    if (entry->display != display) {
      if (admission != nullptr)
        *admission = EglWindowSurfaceAdmission::kWrongDisplay;
      return false;
    }
    auto display_found = displays_.find(display);
    if (display_found != displays_.end()) {
      std::lock_guard<std::mutex> drain_lock(display_found->second->mutex);
      if (display_found->second->phase != DisplayDrain::Phase::kActive &&
          display_found->second->phase != DisplayDrain::Phase::kSucceeded &&
          display_found->second->phase != DisplayDrain::Phase::kFailed) {
        drain = display_found->second;
      }
    }
    if (!entry->PrepareCompletion()) return false;
    const Entry::CloseResult close_result =
        drain == nullptr ? entry->Close() : entry->CloseAndEnroll(drain);
    if (close_result == Entry::CloseResult::kAlreadyClosing) {
      if (admission != nullptr)
        *admission = EglWindowSurfaceAdmission::kClosing;
      return false;
    }
    const bool ready = close_result == Entry::CloseResult::kReady;
    if (admission != nullptr)
      *admission = ready ? EglWindowSurfaceAdmission::kAdmitted
                         : EglWindowSurfaceAdmission::kBusy;
  }
  entry->RunWork(entry->MaybeCleanup());
  return true;
}

bool EglWindowSurfaceOwner::RetireDisplay(
    EglWindowDisplay display, EglWindowDisplayTerminateCallback terminate,
    void* terminate_context) {
  if (display == nullptr || terminate == nullptr) return false;
  std::shared_ptr<DisplayDrain> drain;
  std::vector<std::shared_ptr<Entry>> cohort;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    auto found = displays_.find(display);
    if (found != displays_.end()) {
      std::lock_guard<std::mutex> drain_lock(found->second->mutex);
      if (found->second->phase != DisplayDrain::Phase::kActive ||
          found->second->initialization_reserved) return false;
      drain = found->second;
    }
    // All allocation precedes the admission-closing commit. Failure leaves
    // this display active, with neither unmatched drain counts nor closure.
    try {
      cohort.reserve(entries_.size());
      for (const auto& pair : entries_) {
        if (pair.second->display == display) cohort.push_back(pair.second);
      }
      for (const auto& entry : cohort) {
        if (!entry->PrepareCompletion()) return false;
      }
      if (drain == nullptr) {
        drain = std::make_shared<DisplayDrain>();
        drain->display = display;
        drain->generation = next_generation_++;
        displays_.emplace(display, drain);
      }
    } catch (const std::bad_alloc&) {
      return false;
    } catch (const std::length_error&) {
      return false;
    }
    {
      std::lock_guard<std::mutex> drain_lock(drain->mutex);
      drain->phase = DisplayDrain::Phase::kEnrolling;
      drain->terminate = terminate;
      drain->terminate_context = terminate_context;
    }
    // The exact cohort is captured once while the registry is locked.  No
    // later raw-display enumeration can change the retirement obligation.
    for (const auto& entry : cohort) {
      (void)entry->CloseAndEnroll(drain);
    }
    drain->Seal();
  }
  // Closing happened under the registry lock, so every admitted operation is
  // now either deferred on its lease or ready for immediate cleanup.
  for (const auto& entry : cohort) entry->RunWork(entry->MaybeCleanup());
  drain->TryTerminate();
  return true;
}

EglWindowSurfaceTermination EglWindowSurfaceOwner::TerminationStatus(
    EglWindowDisplay display) const {
  std::shared_ptr<DisplayDrain> drain;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    auto found = displays_.find(display);
    if (found == displays_.end()) return EglWindowSurfaceTermination::kActive;
    drain = found->second;
  }
  std::lock_guard<std::mutex> lock(drain->mutex);
  switch (drain->phase) {
    case DisplayDrain::Phase::kSucceeded:
      return EglWindowSurfaceTermination::kSucceeded;
    case DisplayDrain::Phase::kFailed:
      return EglWindowSurfaceTermination::kFailed;
    case DisplayDrain::Phase::kEnrolling:
    case DisplayDrain::Phase::kDraining:
    case DisplayDrain::Phase::kTerminating:
      return EglWindowSurfaceTermination::kPending;
    case DisplayDrain::Phase::kActive:
      return EglWindowSurfaceTermination::kActive;
  }
  return EglWindowSurfaceTermination::kActive;
}

void EglWindowSurfaceOwner::FinalizeEntry(Entry* entry) {
  if (entry == nullptr) return;
  std::lock_guard<std::mutex> lock(mutex_);
  auto found = entries_.find(entry->surface);
  if (found != entries_.end() && found->second.get() == entry) entries_.erase(found);
}

}  // namespace darwin_art::graphics
