#pragma once
#include <string>

namespace darwin_art::filesystem {
// Acquires one provider lease. Caller releases it through release_filesystem
// (or transfers it to the native resource owner). full_root is the configured
// host directory for guest /, never a library search path or /system subtree.
// The provider owns installed/borrowed process lifetime; this adapter only owns
// the temporary host fd. A missing fd cannot authorize a fresh installation.
bool AcquireProcessAuthority(const char* full_root, std::string* error);
// Stack ownership for failures before the native resource owner exists.
class ProcessAuthorityLease {
 public:
  ProcessAuthorityLease() = default;
  ~ProcessAuthorityLease();
  ProcessAuthorityLease(const ProcessAuthorityLease&) = delete;
  ProcessAuthorityLease& operator=(const ProcessAuthorityLease&) = delete;
  bool Acquire(const char* full_root, std::string* error);
  // Recipient must consume on both success and failure (AttachNativeOwner does).
  void Transfer();
 private:
  bool held_ = false;
};
}
