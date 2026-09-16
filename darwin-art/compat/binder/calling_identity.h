#pragma once
#include <cstdint>

namespace darwin_art::binder {
struct Identity {
  int32_t pid;
  int32_t uid;
  bool explicit_identity;
  bool remote;
};
Identity CurrentIdentity();
int64_t ClearIdentity();
void RestoreIdentity(int64_t token);
class IncomingIdentity final {
 public:
  IncomingIdentity(int32_t pid, int32_t android_uid);
  ~IncomingIdentity();
  IncomingIdentity(const IncomingIdentity&) = delete;
  IncomingIdentity& operator=(const IncomingIdentity&) = delete;
 private:
  Identity previous_;
};
}
