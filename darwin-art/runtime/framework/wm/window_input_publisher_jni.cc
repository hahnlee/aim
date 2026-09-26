#include "window_input_publisher_jni.h"
#include "window_input_endpoint_lease.h"

#include "../input/channel_endpoint.h"
#include "../input/input_channel_jni.h"
#include "../input/input_channel_jni_resources.h"

#include <cstdlib>
#include <iostream>

namespace darwin_art::framework::wm {
namespace {
using EndpointLease = WindowInputEndpointLease;
std::shared_ptr<EndpointLease> PublicationLeases() {
  // System-process WMS lease authority. Tokens never transfer Android policy
  // into this provider and never alias a successor Java channel.
  static const auto owner = EndpointLease::Create();
  return owner;
}

jlong AcquirePublicationLease(JNIEnv* env, jclass, jobject channel) {
  const auto resources = input::AcquireServerInputChannelResources(env, channel);
  if (resources == nullptr || env->ExceptionCheck()) return 0;
  const auto owner = PublicationLeases();
  if (owner == nullptr) {
    input::ThrowInputChannelOutOfMemory(env);
    return 0;
  }
  const auto result = owner->Acquire(resources);
  if (result.status == WindowInputEndpointLeaseAcquireStatus::kOutOfMemory)
    input::ThrowInputChannelOutOfMemory(env);
  return static_cast<jlong>(result.token);
}

jboolean ReleasePublicationLease(JNIEnv*, jclass, jlong token) {
  const auto owner = PublicationLeases();
  return owner != nullptr && owner->Release(static_cast<uint64_t>(token))
      ? JNI_TRUE : JNI_FALSE;
}

jboolean TerminatePublicationLeaseAndQuiesce(JNIEnv*, jclass, jlong token) {
  const auto owner = PublicationLeases();
  return owner != nullptr && owner->TerminateAndQuiesce(static_cast<uint64_t>(token))
      ? JNI_TRUE : JNI_FALSE;
}

jint PublicationStatus(input::InputTransportStatus status) {
  switch (status) {
    case input::InputTransportStatus::kAccepted: return 0;
    case input::InputTransportStatus::kBackpressured: return 1;
    case input::InputTransportStatus::kTerminal: return 2;
  }
  return 2;
}

jint FlushPublicationLease(JNIEnv*, jclass, jlong token) {
  const auto owner = PublicationLeases();
  return owner == nullptr ? 2
                          : PublicationStatus(owner->Flush(static_cast<uint64_t>(token)));
}

jint QueryAcceptedPublicationLeaseTx(JNIEnv*, jclass, jlong token) {
  const auto owner = PublicationLeases();
  if (owner == nullptr) return 3;
  switch (owner->QueryAcceptedTx(static_cast<uint64_t>(token))) {
    case input::InputTransportTxFenceStatus::kFlushed: return 0;
    case input::InputTransportTxFenceStatus::kPending: return 1;
    case input::InputTransportTxFenceStatus::kTerminal: return 2;
    case input::InputTransportTxFenceStatus::kInvalid: return 3;
  }
  return 3;
}

jint PublishLeasedWindow(JNIEnv*, jclass, jlong token, jint left, jint top,
                         jint right, jint bottom, jboolean visible, jint input_flags) {
  const auto owner = PublicationLeases();
  return owner == nullptr ? 2 : PublicationStatus(owner->PublishWindow(
      static_cast<uint64_t>(token), left, top, right, bottom, visible == JNI_TRUE,
      static_cast<uint32_t>(input_flags)));
}

jint PublishLeasedFocus(JNIEnv*, jclass, jlong token, jlong epoch,
                        jboolean focused) {
  if (epoch <= 0) return 2;
  const auto owner = PublicationLeases();
  return owner == nullptr ? 2 : PublicationStatus(owner->PublishFocus(
      static_cast<uint64_t>(token), static_cast<uint64_t>(epoch), focused == JNI_TRUE));
}

jint PublishWindow(JNIEnv* env, jclass, jobject input_channel, jint left,
                   jint top, jint right, jint bottom, jboolean visible,
                   jint input_flags) {
  const auto resources = input::AcquireServerInputChannelResources(env, input_channel);
  if (resources == nullptr || env->ExceptionCheck()) return 2;
  const auto endpoint = resources->Endpoint();
  const auto transport = endpoint == nullptr ? nullptr : endpoint->Transport();
  if (transport == nullptr || transport->ReadFd() < 0) return 2;
  const auto status = endpoint->PublishWindow(left, top, right, bottom,
                                             visible == JNI_TRUE,
                                             static_cast<uint32_t>(input_flags));
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::cerr << "ART Android WMS InputWindow publish name=" << resources->Name()
              << " visible=" << (visible == JNI_TRUE ? 1 : 0) << " frame=["
              << left << "," << top << "," << right << "," << bottom
              << "] status=" << static_cast<int>(status) << "\n";
  }
  return PublicationStatus(status);
}

jint PublishFocus(JNIEnv* env, jclass, jobject input_channel, jlong epoch,
                  jboolean focused) {
  if (epoch <= 0) {
    if (!env->ExceptionCheck()) {
      jclass type = env->FindClass("java/lang/IllegalArgumentException");
      if (type != nullptr) {
        env->ThrowNew(type, "focus epoch must be positive");
        env->DeleteLocalRef(type);
      }
    }
    return 2;
  }
  const auto resources = input::AcquireServerInputChannelResources(env, input_channel);
  if (resources == nullptr || env->ExceptionCheck()) return 2;
  const auto endpoint = resources->Endpoint();
  if (endpoint == nullptr) return 2;
  return PublicationStatus(endpoint->PublishFocus(static_cast<uint64_t>(epoch),
                                                   focused == JNI_TRUE));
}
}  // namespace

bool RegisterWindowInputPublisherNatives(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck()) return false;
  jclass type = env->FindClass("dev/darwinart/runtime/wm/WindowInputPublisher");
  if (type == nullptr || env->ExceptionCheck()) {
    if (type != nullptr) env->DeleteLocalRef(type);
    return false;
  }
  JNINativeMethod methods[]{
      {const_cast<char*>("nativePublish"),
       const_cast<char*>("(Landroid/view/InputChannel;IIIIZI)I"),
       reinterpret_cast<void*>(&PublishWindow)},
      {const_cast<char*>("nativePublishFocus"),
       const_cast<char*>("(Landroid/view/InputChannel;JZ)I"),
       reinterpret_cast<void*>(&PublishFocus)},
      {const_cast<char*>("nativeAcquireLease"),
       const_cast<char*>("(Landroid/view/InputChannel;)J"),
       reinterpret_cast<void*>(&AcquirePublicationLease)},
      {const_cast<char*>("nativeReleaseLease"), const_cast<char*>("(J)Z"),
       reinterpret_cast<void*>(&ReleasePublicationLease)},
      {const_cast<char*>("nativeTerminateLeaseAndQuiesce"), const_cast<char*>("(J)Z"),
       reinterpret_cast<void*>(&TerminatePublicationLeaseAndQuiesce)},
      {const_cast<char*>("nativePublishLease"), const_cast<char*>("(JIIIIZI)I"),
       reinterpret_cast<void*>(&PublishLeasedWindow)},
      {const_cast<char*>("nativePublishFocusLease"), const_cast<char*>("(JJZ)I"),
       reinterpret_cast<void*>(&PublishLeasedFocus)},
      {const_cast<char*>("nativeFlushLease"), const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&FlushPublicationLease)},
      {const_cast<char*>("nativeQueryAcceptedLeaseTx"), const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&QueryAcceptedPublicationLeaseTx)}};
  const bool registered = env->RegisterNatives(type, methods, 9) == JNI_OK &&
                          !env->ExceptionCheck();
  env->DeleteLocalRef(type);
  return registered;
}
}
