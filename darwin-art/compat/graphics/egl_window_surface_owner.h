#pragma once

#include <cstdint>
#include <memory>
#include <mutex>
#include <unordered_map>
#include <vector>

namespace darwin_art::graphics {

using EglWindowHandle = void*;
using EglWindowDisplay = void*;

struct EglWindowSurfaceCreateInfo {
  void* host = nullptr;
  void* native_window = nullptr;
  void* native_buffer = nullptr;
  void* config = nullptr;
  std::int32_t bind_target = 0;
  void* iosurface = nullptr;
  void* iosurface_target = nullptr;
  std::uint32_t texture_target = 0;
  std::uint32_t texture = 0;
  std::uint32_t framebuffer = 0;
  std::uint32_t render_width = 0;
  std::uint32_t render_height = 0;
  std::uint32_t width = 0;
  std::uint32_t height = 0;
  bool target_bound = false;
  bool tex_image_bound = false;
  bool owns_iosurface_ref = false;

  EglWindowSurfaceCreateInfo() = default;
  EglWindowSurfaceCreateInfo(const EglWindowSurfaceCreateInfo&) = delete;
  EglWindowSurfaceCreateInfo& operator=(const EglWindowSurfaceCreateInfo&) = delete;
  EglWindowSurfaceCreateInfo(EglWindowSurfaceCreateInfo&&) noexcept = default;
  EglWindowSurfaceCreateInfo& operator=(EglWindowSurfaceCreateInfo&&) noexcept = default;
};

struct EglWindowSurfaceSnapshot {
  void* host = nullptr;
  void* native_window = nullptr;
  void* native_buffer = nullptr;
  void* config = nullptr;
  std::int32_t bind_target = 0;
  void* iosurface = nullptr;
  void* iosurface_target = nullptr;
  std::uint32_t texture_target = 0;
  std::uint32_t texture = 0;
  std::uint32_t framebuffer = 0;
  std::uint32_t render_width = 0;
  std::uint32_t render_height = 0;
  std::uint32_t width = 0;
  std::uint32_t height = 0;
  bool target_bound = false;
  bool tex_image_bound = false;
  bool owns_iosurface_ref = false;
};

struct EglWindowSurfaceCleanup {
  EglWindowDisplay display = nullptr;
  EglWindowHandle surface = nullptr;
  EglWindowSurfaceSnapshot resources;
};
// The callback runs after the last admitted operation, outside owner locks,
// on the thread releasing that operation.  A backend that requires a current
// EGL context must perform a synchronous dispatch to its context owner before
// returning from this callback.
using EglWindowSurfaceCleanupCallback = void (*)(
    const EglWindowSurfaceCleanup&, void*);
using EglWindowDisplayTerminateCallback = bool (*)(EglWindowDisplay, void*);

enum class EglWindowSurfaceAdmission : std::uint8_t {
  kAdmitted,
  kUnknown,
  kBusy,
  kClosing,
  kWrongDisplay,
  kTerminating,
};

enum class EglWindowSurfaceTermination : std::uint8_t {
  kActive,
  kPending,
  kSucceeded,
  kFailed,
};

class EglWindowSurfaceOwner final {
 private:
  struct Entry;
  struct DisplayDrain;
  struct OwnerControl;

 public:
  class DisplayInitializationLease final {
   public:
    DisplayInitializationLease() = default;
    DisplayInitializationLease(DisplayInitializationLease&&) noexcept;
    DisplayInitializationLease& operator=(DisplayInitializationLease&&) noexcept;
    DisplayInitializationLease(const DisplayInitializationLease&) = delete;
    DisplayInitializationLease& operator=(const DisplayInitializationLease&) = delete;
    ~DisplayInitializationLease();
    explicit operator bool() const { return drain_ != nullptr; }
    // Publishes the generation only after the backend initialize call succeeds.
    // Failure/abandonment restores the exact prior lifecycle state.
    bool Complete(bool backend_succeeded) noexcept;

   private:
    DisplayInitializationLease(std::shared_ptr<OwnerControl> control,
                               std::shared_ptr<DisplayDrain> drain,
                               std::shared_ptr<DisplayDrain> previous,
                               bool restore_active);
    std::shared_ptr<OwnerControl> control_;
    std::shared_ptr<DisplayDrain> drain_;
    std::shared_ptr<DisplayDrain> previous_;
    bool restore_active_ = false;
    bool completed_ = false;
    friend class EglWindowSurfaceOwner;
  };

  class Lease final {
   public:
    Lease() = default;
    Lease(Lease&&) noexcept;
    Lease& operator=(Lease&&) noexcept;
    Lease(const Lease&) = delete;
    Lease& operator=(const Lease&) = delete;
    ~Lease();

    explicit operator bool() const { return entry_ != nullptr; }
    EglWindowSurfaceSnapshot Snapshot() const;
    void SetTextureAndFramebuffer(std::uint32_t texture,
                                  std::uint32_t framebuffer,
                                  bool target_bound, bool tex_image_bound);
    void SetIosurfaceTarget(void* iosurface, void* target,
                            std::uint32_t width, std::uint32_t height,
                            bool owns_ref);
    void SetNativeBuffer(void* buffer);
    void SetDimensions(std::uint32_t render_width,
                       std::uint32_t render_height,
                       std::uint32_t width, std::uint32_t height);

   private:
    explicit Lease(std::shared_ptr<Entry> entry);
    std::shared_ptr<Entry> entry_;
    friend class EglWindowSurfaceOwner;
  };

  static EglWindowSurfaceOwner& Instance();
  DisplayInitializationLease ReserveDisplayInitialization(
      EglWindowDisplay display);
  // Returns true only for an active generation. Pending/terminating display
  // generations are never advertised as reopened by an initialize caller.
  bool BeginDisplay(EglWindowDisplay display);
  // Read-only state query used to classify window creation. It has no registry
  // side effects and never reserves or admits a surface.
  bool IsDisplayInitialized(EglWindowDisplay display) const;
  bool Create(EglWindowDisplay display, EglWindowHandle surface,
              EglWindowSurfaceCreateInfo resources,
              EglWindowSurfaceCleanupCallback cleanup, void* cleanup_context,
              bool* resources_cleaned = nullptr);
  Lease Acquire(EglWindowDisplay display, EglWindowHandle surface,
                EglWindowSurfaceAdmission* admission = nullptr);
  Lease Swap(EglWindowDisplay display, EglWindowHandle surface,
             EglWindowSurfaceAdmission* admission = nullptr);
  bool Destroy(EglWindowDisplay display, EglWindowHandle surface,
               EglWindowSurfaceAdmission* admission = nullptr);
  bool RetireDisplay(EglWindowDisplay display,
                     EglWindowDisplayTerminateCallback terminate,
                     void* terminate_context);
  EglWindowSurfaceTermination TerminationStatus(
      EglWindowDisplay display) const;

  EglWindowSurfaceOwner();
  ~EglWindowSurfaceOwner();

 private:
  bool CompleteDisplayInitialization(DisplayInitializationLease* lease,
                                     bool backend_succeeded) noexcept;
  void FinalizeEntry(Entry* entry);
  mutable std::mutex mutex_;
  std::unordered_map<EglWindowHandle, std::shared_ptr<Entry>> entries_;
  std::unordered_map<EglWindowDisplay, std::shared_ptr<DisplayDrain>> displays_;
  std::uint64_t next_generation_ = 1;
  std::shared_ptr<OwnerControl> control_;
};

}  // namespace darwin_art::graphics
