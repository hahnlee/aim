#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <string>
#include <vector>

#include "darwin_jni_shorty.h"
#include "jni/android_varargs.h"
#include "jni/method_call.h"
#include "darwin_runtime_adapters_internal.h"

namespace android {

namespace {

bool LookupArguments(ElfLibrary *library, void *method, void *raw_args,
                     std::vector<jvalue> *output) {
  if (library == nullptr || method == nullptr)
    return false;
  std::string descriptor;
  {
    std::lock_guard<std::mutex> lock(library->method_descriptor_mutex);
    const auto found = library->method_descriptors.find(method);
    if (found == library->method_descriptors.end())
      return false;
    descriptor = found->second;
  }
  return darwin_art::jni::DecodeAndroidArguments(descriptor, raw_args, output);
}

// Android native libraries own every thread that they attach to ART. Some
// libraries rely on their Android pthread entry wrapper to detach that thread
// on return instead of pairing every JavaVM::AttachCurrentThread call at the
// call site. Our translated pthread entry cannot see the JavaVM hidden behind
// the guest JNI proxy, so retain that ownership here and release it from the
// host thread's C++ TLS teardown. This runs before ART's pthread-key teardown,
// which intentionally aborts when an attached native thread disappears.
//
// Only attachments made by this proxy are owned. Java-created threads and a
// thread that was already attached before entering the guest remain under
// their original owner's control.
class ProxyThreadAttachment {
public:
  ~ProxyThreadAttachment() { DetachIfOwned(); }

  void Arm(JavaVM *vm) {
    if (vm_ == nullptr)
      vm_ = vm;
  }

  void Disarm(JavaVM *vm) {
    if (vm_ == vm)
      vm_ = nullptr;
  }

private:
  void DetachIfOwned() {
    JavaVM *vm = vm_;
    vm_ = nullptr;
    if (vm == nullptr)
      return;
    JNIEnv *env = nullptr;
    if (vm->GetEnv(reinterpret_cast<void **>(&env), JNI_VERSION_1_6) == JNI_OK)
      (void)vm->DetachCurrentThread();
  }

  JavaVM *vm_ = nullptr;
};

ProxyThreadAttachment &CurrentProxyThreadAttachment() {
  thread_local ProxyThreadAttachment attachment;
  return attachment;
}

} // namespace

void *ProxyCurrentEnv(void *) { return CurrentArtEnv(); }

int32_t ProxyAttachCurrentThread(void *context, void *arguments,
                                 int32_t as_daemon) {
  auto *library = static_cast<ElfLibrary *>(context);
  if (library == nullptr || library->art_vm == nullptr)
    return JNI_ERR;
  JNIEnv *previous_env = nullptr;
  const jint previous = library->art_vm->GetEnv(
      reinterpret_cast<void **>(&previous_env), JNI_VERSION_1_6);
  if (previous != JNI_OK && previous != JNI_EDETACHED)
    return previous;
  JNIEnv *env = nullptr;
  const jint result =
      as_daemon != 0
          ? library->art_vm->AttachCurrentThreadAsDaemon(&env, arguments)
          : library->art_vm->AttachCurrentThread(&env, arguments);
  if (result == JNI_OK && previous == JNI_EDETACHED)
    CurrentProxyThreadAttachment().Arm(library->art_vm);
  return result;
}

int32_t ProxyDetachCurrentThread(void *context) {
  auto *library = static_cast<ElfLibrary *>(context);
  if (library == nullptr || library->art_vm == nullptr)
    return JNI_ERR;
  const jint result = library->art_vm->DetachCurrentThread();
  if (result == JNI_OK)
    CurrentProxyThreadAttachment().Disarm(library->art_vm);
  return result;
}

void *ProxyFindClass(void *context, const char *name) {
  (void)context;
  JNIEnv *art_env = CurrentArtEnv();
  if (art_env == nullptr || name == nullptr) {
    return nullptr;
  }
  // ART selects the nativeLoad override, current method's loader or system
  // loader. Keep its slash-name contract and pending exception intact.
  void *clazz = art_env->FindClass(name);
  return clazz;
}

void *ProxyGetMethodId(void *context, void *clazz, const char *name,
                       const char *signature, int32_t is_static) {
  auto *library = static_cast<ElfLibrary *>(context);
  JNIEnv *art_env = CurrentArtEnv();
  if (library == nullptr || art_env == nullptr || clazz == nullptr ||
      name == nullptr || signature == nullptr) {
    return nullptr;
  }
  jmethodID method =
      is_static != 0
          ? art_env->GetStaticMethodID(static_cast<jclass>(clazz), name,
                                       signature)
          : art_env->GetMethodID(static_cast<jclass>(clazz), name, signature);
  if (method != nullptr) {
    std::lock_guard<std::mutex> lock(library->method_descriptor_mutex);
    library->method_descriptors[method] = signature;
  }
  if (std::getenv("DARWIN_ART_DEBUG_JNI_CALLS") != nullptr) {
    std::cerr << "DARWIN JNI method name=" << name
              << " signature=" << signature
              << " static=" << is_static << " id=" << method << "\n";
  }
  return method;
}

uint64_t ProxyCallMethodV(void *context, void *object, void *method,
                          void *android_va_list, int32_t return_shorty,
                          int32_t is_static) {
  auto *library = static_cast<ElfLibrary *>(context);
  JNIEnv *art_env = CurrentArtEnv();
  std::vector<jvalue> arguments;
  if (art_env == nullptr || object == nullptr ||
      !LookupArguments(library, method, android_va_list, &arguments)) {
    return 0;
  }
  return darwin_art::jni::CallMethodA(
      art_env, static_cast<jobject>(object), static_cast<jmethodID>(method),
      arguments.empty() ? nullptr : arguments.data(), return_shorty, is_static);
}

} // namespace android
