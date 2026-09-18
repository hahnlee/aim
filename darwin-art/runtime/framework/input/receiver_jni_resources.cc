#include "receiver_jni_resources.h"

#include <utility>

namespace darwin_art::input {
void CleanupReceiverRefs(JNIEnv* env, InputReceiver* receiver) {
  if (env == nullptr || receiver == nullptr) return;
  bool expected = false;
  if (!receiver->refs_cleaned.compare_exchange_strong(
          expected, true, std::memory_order_acq_rel)) return;
  if (receiver->weak_receiver != nullptr)
    env->DeleteGlobalRef(std::exchange(receiver->weak_receiver, nullptr));
  if (receiver->view_root != nullptr)
    env->DeleteGlobalRef(std::exchange(receiver->view_root, nullptr));
  if (receiver->original_channel_token != nullptr)
    env->DeleteGlobalRef(std::exchange(receiver->original_channel_token, nullptr));
  (void)receiver->admission.CompleteCleanup();
}

void ReleaseReceiverAdmission(JNIEnv* env, InputReceiver* receiver) {
  if (receiver != nullptr && receiver->admission.Release())
    CleanupReceiverRefs(env, receiver);
}

void RetireReceiverResources(JNIEnv* env,
                            const std::shared_ptr<InputReceiver>& receiver) {
  if (receiver == nullptr) return;
  const bool cleanup_winner = receiver->admission.Retire();
  receiver->disposed.store(true, std::memory_order_release);
  (void)receiver->binding.Retire();
  if (cleanup_winner) CleanupReceiverRefs(env, receiver.get());
}

ReceiverCallbackAdmission::ReceiverCallbackAdmission(
    JNIEnv* env, std::shared_ptr<InputReceiver> receiver)
    : env_(env), receiver_(std::move(receiver)),
      admitted_(env != nullptr && receiver_ != nullptr && receiver_->admission.Admit()) {}

ReceiverCallbackAdmission::~ReceiverCallbackAdmission() {
  if (admitted_) ReleaseReceiverAdmission(env_, receiver_.get());
}

}  // namespace darwin_art::input
