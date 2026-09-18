#include "runtime/framework/input/receiver_jni_resources.h"

#include <cassert>
#include <vector>

using namespace darwin_art::input;

namespace {
struct Observer {
  ReceiverAdmission::RetirementHandle retirement;
  int calls = 0;
};
Observer* observer = nullptr;
std::vector<jobject> deleted;
void DeleteGlobal(JNIEnv*, jobject value) {
  assert(observer->calls == 0);
  deleted.push_back(value);
}
void Quiescent(void* opaque) noexcept {
  auto* state = static_cast<Observer*>(opaque);
  assert(deleted.size() == 3);
  assert(state->retirement.IsQuiescent());
  ++state->calls;
}
}

// FD retirement has separate production-binding tests. This fixture isolates
// the real JNI resource adapter and real admission Control cleanup ordering.
namespace darwin_art::input {
ReceiverEndpointBinding::ReceiverEndpointBinding() = default;
ReceiverEndpointBinding::~ReceiverEndpointBinding() = default;
bool ReceiverEndpointBinding::Retire() { return true; }
}

int main() {
  JNINativeInterface table{};
  table.DeleteGlobalRef = DeleteGlobal;
  JNIEnv env{&table};
  auto receiver = std::make_shared<InputReceiver>();
  receiver->weak_receiver = reinterpret_cast<jobject>(0x101);
  receiver->view_root = reinterpret_cast<jobject>(0x102);
  receiver->original_channel_token = reinterpret_cast<jobject>(0x103);
  auto state = std::make_shared<Observer>();
  state->retirement = receiver->admission.RetainRetirement();
  observer = state.get();
  assert(state->retirement.SetQuiescenceNotification(Quiescent, state));
  assert(receiver->admission.Admit());
  RetireReceiverResources(&env, receiver);
  assert(receiver->disposed && deleted.empty());
  assert(!state->retirement.IsQuiescent());
  ReleaseReceiverAdmission(&env, receiver.get());
  assert(receiver->weak_receiver == nullptr && receiver->view_root == nullptr &&
         receiver->original_channel_token == nullptr);
  assert(receiver->refs_cleaned && state->calls == 1);
  RetireReceiverResources(&env, receiver);
  CleanupReceiverRefs(&env, receiver.get());
  assert(deleted.size() == 3 && state->calls == 1);
  std::weak_ptr<InputReceiver> weak = receiver;
  receiver.reset();
  assert(weak.expired() && state->retirement.IsQuiescent());
}
