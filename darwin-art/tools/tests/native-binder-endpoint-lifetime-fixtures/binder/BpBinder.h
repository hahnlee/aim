#pragma once

#include "IBinder.h"

namespace android {

class BpBinder : public IBinder {
 public:
  BpBinder* remoteBinder() override { return this; }
  virtual bool isRpcBinder() const { return false; }
};

}  // namespace android
