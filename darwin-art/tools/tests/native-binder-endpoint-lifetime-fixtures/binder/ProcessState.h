#pragma once

namespace android {

class ProcessState {
 public:
  static ProcessState* selfOrNull() { return nullptr; }
  bool isThreadPoolStarted() const { return false; }
};

}  // namespace android
