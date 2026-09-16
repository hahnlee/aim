#pragma once

#include <jni.h>
#include "process_attachment.h"

namespace darwin_art::framework::app {

// Runs on the prepared ART main Looper. ApplicationThread constructs its own
// AppBindData and queues core-settings/BIND_APPLICATION in framework order.
// ActivityThread.attach requests binding from system AMS; no local bind dispatch.
inline jobject AwaitApplication(JNIEnv* env) {
  if (env->ExceptionCheck()) return nullptr;
  if (env->PushLocalFrame(64) < 0) return nullptr;
  auto fail = [&]() -> jobject { env->PopLocalFrame(nullptr); return nullptr; };
  jclass type = env->FindClass("android/app/ActivityThread");
  if (type == nullptr) return fail();
  jmethodID current = env->GetStaticMethodID(type, "currentActivityThread",
                                            "()Landroid/app/ActivityThread;");
  if (current == nullptr) return fail();
  jobject thread = env->CallStaticObjectMethod(type, current);
  if (env->ExceptionCheck()) return fail();
  if (thread == nullptr) {
    jmethodID ctor = env->GetMethodID(type, "<init>", "()V");
    if (ctor == nullptr) return fail();
    thread = env->NewObject(type, ctor);
    if (thread == nullptr || !AttachApplicationProcess(env, thread)) return fail();
  }
  jclass looper_type = env->FindClass("android/os/Looper");
  if (looper_type == nullptr) return fail();
  jmethodID my_looper = env->GetStaticMethodID(looper_type, "myLooper", "()Landroid/os/Looper;");
  jmethodID once = env->GetStaticMethodID(looper_type, "loopOnce", "(Landroid/os/Looper;JI)Z");
  jmethodID get_app = env->GetMethodID(type, "getApplication", "()Landroid/app/Application;");
  if (my_looper == nullptr || once == nullptr || get_app == nullptr) return fail();
  jobject looper = env->CallStaticObjectMethod(looper_type, my_looper);
  if (looper == nullptr || env->ExceptionCheck()) return fail();
  jclass binder = env->FindClass("android/os/Binder");
  if (binder == nullptr) return fail();
  jmethodID clear_identity = env->GetStaticMethodID(binder, "clearCallingIdentity", "()J");
  if (clear_identity == nullptr) return fail();
  // AOSP Looper.loop clears once to establish local identity, then again to
  // capture that established token. The first token describes the old state.
  env->CallStaticLongMethod(binder, clear_identity);
  if (env->ExceptionCheck()) return fail();
  const jlong identity = env->CallStaticLongMethod(binder, clear_identity);
  if (env->ExceptionCheck()) return fail();
  while (true) {
    jobject application = env->CallObjectMethod(thread, get_app);
    if (env->ExceptionCheck()) return fail();
    if (application != nullptr) return env->PopLocalFrame(application);
    // System AMS queues BIND_APPLICATION. Use Looper dispatch rather than
    // calling handleBindApplication or pulling messages out of the queue.
    const jboolean running = env->CallStaticBooleanMethod(
        looper_type, once, looper, identity, static_cast<jint>(0));
    if (env->ExceptionCheck() || !running) return fail();
  }
}

}  // namespace darwin_art::framework::app
