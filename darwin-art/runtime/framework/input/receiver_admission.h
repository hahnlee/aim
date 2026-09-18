#pragma once

#include <cstdint>
#include <mutex>
#include <memory>
#include <utility>

namespace darwin_art::input {

// Admission is the per-receiver lifetime boundary. Registry leases protect
// allocation; this gate protects JNI state from retirement races.
class ReceiverAdmission final {
  struct Control;
 public:
  class RetirementHandle final {
   public:
    RetirementHandle() = default;
    // Includes elected JNI cleanup completion, unlike the gate-only query.
    bool IsQuiescent() const;
    // Weak context must be independent of the receiver/JNI allocation.
    // Subscribe before querying; completion may notify immediately.
    bool SetQuiescenceNotification(void (*notify)(void*) noexcept,
                                  std::weak_ptr<void> context);
   private:
    friend class ReceiverAdmission;
    explicit RetirementHandle(std::shared_ptr<Control> control)
        : control_(std::move(control)) {}
    std::shared_ptr<Control> control_;
  };
  ReceiverAdmission();
  ReceiverAdmission(const ReceiverAdmission&) = delete;
  ReceiverAdmission& operator=(const ReceiverAdmission&) = delete;
  bool Admit();
  // Closes new admission. Returns true only for the caller that must perform
  // cleanup, and never while the lifecycle mutex is held.
  bool Retire();
  // Releases one admitted use. Returns true only for the cleanup winner.
  bool Release();
  // Closed admission and zero active uses; cleanup election alone does not
  // establish that a retirement handoff may stop waiting for JNI callbacks.
  bool IsQuiescent() const;
  RetirementHandle RetainRetirement() const;
  // The elected resource adapter calls this only after deleting JNI globals.
  bool CompleteCleanup();

 private:
  static void NotifyQuiescence(const std::shared_ptr<Control>& control);
  const std::shared_ptr<Control> control_;
};

}  // namespace darwin_art::input
