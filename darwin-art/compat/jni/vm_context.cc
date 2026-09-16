#include "vm_context.h"
#include "android_varargs.h"
#include "method_call.h"
#include <new>
#include <utility>
#include <vector>

namespace darwin_art::jni {
namespace {
void AllocationFailure(JNIEnv *env) {
  if (!env || env->ExceptionCheck())
    return;
  jclass error = env->FindClass("java/lang/OutOfMemoryError");
  if (error) {
    env->ThrowNew(error, "JNI ABI callback state allocation failed");
    env->DeleteLocalRef(error);
  }
}
} // namespace

VmContext::VmContext(JavaVM *vm,
                     std::shared_ptr<NativeRegistration> registration)
    : vm_(vm), registration_(std::move(registration)) {}

std::shared_ptr<VmContext>
VmContext::Create(JavaVM *vm,
                  std::shared_ptr<NativeRegistration> registration) {
  if (!vm || !registration)
    return nullptr;
  return std::shared_ptr<VmContext>(new VmContext(vm, std::move(registration)));
}

JNIEnv *VmContext::Environment() const {
  void *env = nullptr;
  return vm_->GetEnv(&env, JNI_VERSION_1_6) == JNI_OK
             ? static_cast<JNIEnv *>(env)
             : nullptr;
}

void *VmContext::CurrentEnvironment(void *raw) noexcept {
  return raw ? static_cast<VmContext *>(raw)->Environment() : nullptr;
}

int32_t VmContext::Attach(void *raw, void *arguments, int32_t daemon) noexcept {
  if (!raw || (daemon != 0 && daemon != 1))
    return JNI_ERR;
  JNIEnv *env = nullptr;
  auto *self = static_cast<VmContext *>(raw);
  return daemon ? self->vm_->AttachCurrentThreadAsDaemon(&env, arguments)
                : self->vm_->AttachCurrentThread(&env, arguments);
}

int32_t VmContext::Detach(void *raw) noexcept {
  return raw ? static_cast<VmContext *>(raw)->vm_->DetachCurrentThread()
             : JNI_ERR;
}

void *VmContext::FindClass(void *raw, const char *name) noexcept {
  auto *env = static_cast<JNIEnv *>(CurrentEnvironment(raw));
  return env && name ? env->FindClass(name) : nullptr;
}

int32_t VmContext::Register(void *raw, void *clazz,
                            const DarwinArtJniNativeMethod *methods,
                            int32_t count) noexcept {
  auto *env = static_cast<JNIEnv *>(CurrentEnvironment(raw));
  if (!env || !clazz || count < 0 || (count && !methods))
    return JNI_ERR;
  return static_cast<VmContext *>(raw)->registration_->Register(
      env, static_cast<jclass>(clazz), methods, count);
}

int32_t VmContext::ThrowNew(void *raw, void *clazz, const char *message) noexcept {
  auto *env = static_cast<JNIEnv *>(CurrentEnvironment(raw));
  return env && clazz ? env->ThrowNew(static_cast<jclass>(clazz), message)
                      : JNI_ERR;
}

void *VmContext::GetMethod(void *raw, void *clazz, const char *name,
                           const char *signature, int32_t is_static) noexcept {
  auto *env = static_cast<JNIEnv *>(CurrentEnvironment(raw));
  if (!env || !clazz || !name || !signature ||
      (is_static != 0 && is_static != 1))
    return nullptr;
  // ART validates the descriptor and selects the method. Never clear its
  // exception or probe a second invocation kind after a failed lookup.
  jmethodID method =
      is_static
          ? env->GetStaticMethodID(static_cast<jclass>(clazz), name, signature)
          : env->GetMethodID(static_cast<jclass>(clazz), name, signature);
  if (method) {
    auto *self = static_cast<VmContext *>(raw);
    try {
      std::lock_guard lock(self->methods_mutex_);
      self->descriptors_[method] = signature;
    } catch (const std::bad_alloc &) {
      AllocationFailure(env);
      return nullptr;
    }
  }
  return method;
}

uint64_t VmContext::CallMethod(void *raw, void *object, void *method,
                               void *args, int32_t result, int32_t kind) noexcept {
  auto *env = static_cast<JNIEnv *>(CurrentEnvironment(raw));
  if (!env || !object || !method)
    return 0;
  auto *self = static_cast<VmContext *>(raw);
  try {
    std::string descriptor;
    {
      std::lock_guard lock(self->methods_mutex_);
      auto found = self->descriptors_.find(static_cast<jmethodID>(method));
      if (found == self->descriptors_.end())
        return 0;
      descriptor = found->second;
    }
    std::vector<jvalue> values;
    if (!DecodeAndroidArguments(descriptor, args, &values))
      return 0;
    // No locks across a Java call: it may reenter method lookup/registration.
    return CallMethodA(env, static_cast<jobject>(object),
                       static_cast<jmethodID>(method),
                       values.empty() ? nullptr : values.data(), result, kind);
  } catch (const std::bad_alloc &) {
    AllocationFailure(env);
    return 0;
  }
}

DarwinArtJniBackend VmContext::Backend() {
  return {this,     CurrentEnvironment, Attach,    Detach,    FindClass,
          Register, ThrowNew,           GetMethod, CallMethod};
}
} // namespace darwin_art::jni
