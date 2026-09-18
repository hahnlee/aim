#pragma once

#include "receiver_routing_lifecycle.h"

#include <jni.h>
#include <memory>

namespace darwin_art::input {
struct InputReceiver;
class ChannelEndpoint;

// Coordinates the Android receiver epoch; the driver retains only transport
// resources, never the receiver, channel wrapper, or Java references.
bool PrepareReceiverRetirement(
    InputReceiver& receiver, InputRoutingRecipientHandle recipient,
    std::shared_ptr<ChannelEndpoint> endpoint, InputRoutingHandle routing,
    bool (*refresh_writable)(const InputRoutingHandle&));
ReceiverRoutingPublishResult PublishReceiverRetirement(InputReceiver& receiver);
bool CloseReceiverRetirement(InputReceiver& receiver) noexcept;

// Initialization rollback belongs to lifecycle coordination, not the JNI
// resource adapter. Exact routing closure precedes registry/resource release.
class ReceiverInitializationAdmission final {
 public:
  ReceiverInitializationAdmission(JNIEnv* env,
                                  std::shared_ptr<InputReceiver> receiver);
  ~ReceiverInitializationAdmission();
  bool Admitted() const { return admitted_; }
  void Commit();
  ReceiverInitializationAdmission(const ReceiverInitializationAdmission&) = delete;
  ReceiverInitializationAdmission& operator=(const ReceiverInitializationAdmission&) = delete;
 private:
  JNIEnv* env_;
  const std::shared_ptr<InputReceiver> receiver_;
  bool admitted_;
  bool committed_ = false;
};
}  // namespace darwin_art::input
