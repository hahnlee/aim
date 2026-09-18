#include "system_class_loader.h"

#include <iostream>

#include <jni.h>

#include "jni/java_vm_ext.h"
#include "runtime.h"
#include "scoped_thread_state_change-inl.h"

extern "C" int darwin_art_install_context_loader(JNIEnv* env,
                                                 jobject app_loader) {
  if (env == nullptr || app_loader == nullptr) return 4;
  // ART's normal app_process startup installs the application PathClassLoader
  // as both the process system loader and the current thread context loader.
  // Our native ClassLinker helper has already created the canonical loader, so
  // publish that same object instead of leaving ClassLoader's lazy boot-only
  // fallback in the parent chain of application-defined loaders.
  jclass system_loader_holder =
      env->FindClass("java/lang/ClassLoader$SystemClassLoader");
  jfieldID system_loader_field =
      system_loader_holder == nullptr
          ? nullptr
          : env->GetStaticFieldID(system_loader_holder, "loader",
                                  "Ljava/lang/ClassLoader;");
  if (system_loader_field == nullptr || env->ExceptionCheck()) {
    std::cerr << "ART Darwin DEX: system ClassLoader lookup failed\n";
    env->ExceptionClear();
    env->DeleteLocalRef(system_loader_holder);
    return 4;
  }
  env->SetStaticObjectField(system_loader_holder, system_loader_field,
                            app_loader);
  env->DeleteLocalRef(system_loader_holder);
  {
    art::ScopedObjectAccess soa(env);
    art::Runtime* runtime = art::Runtime::Current();
    jobject process_loader = runtime->GetJavaVM()->AddGlobalRef(
        soa.Self(), soa.Decode<art::mirror::Object>(app_loader));
    if (process_loader == nullptr) return 4;
    jobject old_loader =
        runtime->SetSystemClassLoaderForAppProcess(process_loader);
    soa.Self()->SetClassLoaderOverride(process_loader);
    if (!env->IsSameObject(runtime->GetSystemClassLoader(), app_loader)) {
      std::cerr << "ART Darwin DEX: process ClassLoader publication mismatch\n";
      return 4;
    }
    if (old_loader != nullptr) {
      runtime->GetJavaVM()->DeleteGlobalRef(soa.Self(), old_loader);
    }
  }
  jclass thread_class = env->FindClass("java/lang/Thread");
  jmethodID current_thread =
      thread_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(thread_class, "currentThread",
                                   "()Ljava/lang/Thread;");
  jmethodID set_context_loader =
      thread_class == nullptr
          ? nullptr
          : env->GetMethodID(thread_class, "setContextClassLoader",
                             "(Ljava/lang/ClassLoader;)V");
  jobject managed_thread =
      current_thread == nullptr
          ? nullptr
          : env->CallStaticObjectMethod(thread_class, current_thread);
  if (managed_thread == nullptr || set_context_loader == nullptr ||
      env->ExceptionCheck()) {
    std::cerr << "ART Darwin DEX: context ClassLoader setup failed\n";
    if (managed_thread != nullptr) env->DeleteLocalRef(managed_thread);
    if (thread_class != nullptr) env->DeleteLocalRef(thread_class);
    env->ExceptionClear();
    return 4;
  }
  env->CallVoidMethod(managed_thread, set_context_loader, app_loader);
  env->DeleteLocalRef(managed_thread);
  env->DeleteLocalRef(thread_class);
  if (env->ExceptionCheck()) {
    std::cerr << "ART Darwin DEX: context ClassLoader install failed\n";
    env->ExceptionClear();
    return 4;
  }
  return 0;
}
