#include "runtime_app_bootstrap.h"
#include "runtime_acceptance_state.h"

#include "../runtime/art/system_class_loader.h"

#include <cstring>
#include <cerrno>
#include <cstdlib>
#include <iostream>
#include <memory>
#include <string>
#include <unistd.h>
#include <vector>

#include "base/logging.h"
#include "class_linker.h"
#include "dex/art_dex_file_loader.h"
#include "handle_scope-inl.h"
#include "jni/java_vm_ext.h"
#include "mirror/class-inl.h"
#include "mirror/class_loader.h"
#include "runtime.h"
#include "scoped_thread_state_change-inl.h"
#include "thread-current-inl.h"

namespace darwin_art_app {

int load_classes(JNIEnv* env,
                 art::Thread* self,
                 art::ClassLinker* class_linker,
                 art::ScopedObjectAccess& soa,
                 art::StackHandleScope<32>& hs,
                 bool run_apk_app,
                 const char* app_dex,
                 const char* support_dex,
                 const char* native_library_path,
                 const char* activity_descriptor,
                 bool run_direct_apk,
                 const char* direct_apk_path,
                 bool run_elf_jni_fixture,
                 bool run_network_acceptance,
                 bool headless_fixture,
                 ClassSet* out) {
  if (out == nullptr || self == nullptr || class_linker == nullptr ||
      app_dex == nullptr || activity_descriptor == nullptr) {
    return 3;
  }

  // Android's launcher publishes one system PathClassLoader for the process
  // class path. RuntimeArgumentMap::ClassPath has already created that loader;
  // wrapping it in another PathClassLoader duplicates the DexFile and splits
  // defining classes from later JVMTI/addDexPath mutations.
  jclass class_loader_class = env->FindClass("java/lang/ClassLoader");
  jmethodID get_system_loader =
      class_loader_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(class_loader_class, "getSystemClassLoader",
                                   "()Ljava/lang/ClassLoader;");
  jobject managed_loader =
      get_system_loader == nullptr
          ? nullptr
          : env->CallStaticObjectMethod(class_loader_class, get_system_loader);
  env->DeleteLocalRef(class_loader_class);
  if (managed_loader == nullptr || self->IsExceptionPending()) {
    std::cerr << "ART Darwin DEX: Java PathClassLoader creation failed\n";
    return 4;
  }
  art::Handle<art::mirror::ClassLoader> app_loader =
      hs.NewHandle(soa.Decode<art::mirror::ClassLoader>(managed_loader));
  out->app_loader = managed_loader;
  // Publish the process PathClassLoader immediately. Resource/bootstrap
  // classes can initialize before load_classes returns and need to resolve
  // the process-local Darwin Binder service bridge through this loader.
  if (darwin_art_install_context_loader(env, managed_loader) != 0) {
    std::cerr << "ART Darwin DEX: early context ClassLoader install failed\n";
    return 4;
  }
  std::cerr << "ART Darwin DEX: application ClassLoader="
            << static_cast<void*>(app_loader.Get()) << "\n";
  if (std::getenv("DARWIN_ART_TRACE_DEX_IDENTITY") != nullptr) {
    jclass base_loader = env->FindClass("dalvik/system/BaseDexClassLoader");
    jclass dex_path_list = env->FindClass("dalvik/system/DexPathList");
    jclass dex_element =
        env->FindClass("dalvik/system/DexPathList$Element");
    jclass dex_file = env->FindClass("dalvik/system/DexFile");
    jfieldID path_list_field = base_loader == nullptr
                                   ? nullptr
                                   : env->GetFieldID(
                                         base_loader, "pathList",
                                         "Ldalvik/system/DexPathList;");
    jfieldID elements_field = dex_path_list == nullptr
                                  ? nullptr
                                  : env->GetFieldID(
                                        dex_path_list, "dexElements",
                                        "[Ldalvik/system/DexPathList$Element;");
    jfieldID dex_field = dex_element == nullptr
                             ? nullptr
                             : env->GetFieldID(dex_element, "dexFile",
                                               "Ldalvik/system/DexFile;");
    jfieldID cookie_field = dex_file == nullptr
                                ? nullptr
                                : env->GetFieldID(dex_file, "mCookie",
                                                  "Ljava/lang/Object;");
    jobject path_list = path_list_field == nullptr
                            ? nullptr
                            : env->GetObjectField(managed_loader, path_list_field);
    jobjectArray elements = path_list == nullptr || elements_field == nullptr
                                ? nullptr
                                : static_cast<jobjectArray>(
                                      env->GetObjectField(path_list, elements_field));
    const jsize count = elements == nullptr ? 0 : env->GetArrayLength(elements);
    std::cerr << "ART Darwin DEX identity: path_elements=" << count << "\n";
    for (jsize index = 0; index < count; ++index) {
      jobject element = env->GetObjectArrayElement(elements, index);
      jobject dex = element == nullptr || dex_field == nullptr
                        ? nullptr
                        : env->GetObjectField(element, dex_field);
      jobject cookie = dex == nullptr || cookie_field == nullptr
                           ? nullptr
                           : env->GetObjectField(dex, cookie_field);
      std::cerr << "ART Darwin DEX identity: index=" << index
                << " dex=" << static_cast<void*>(dex)
                << " cookie=" << static_cast<void*>(cookie) << "\n";
      env->DeleteLocalRef(cookie);
      env->DeleteLocalRef(dex);
      env->DeleteLocalRef(element);
    }
    env->DeleteLocalRef(elements);
    env->DeleteLocalRef(path_list);
    env->DeleteLocalRef(dex_file);
    env->DeleteLocalRef(dex_element);
    env->DeleteLocalRef(dex_path_list);
    env->DeleteLocalRef(base_loader);
    if (env->ExceptionCheck()) env->ExceptionClear();
  }

  // Android's app_process adds secondary code through BaseDexClassLoader's
  // addDexPath before framework classes begin resolving. RuntimeArgumentMap is
  // already consumed by Runtime::Create, so changing the string later cannot
  // update this loader. Use the same hidden API for the compatibility service
  // DEX and preserve the primary APK/probe DEX unchanged.
  if (support_dex != nullptr && support_dex[0] != '\0' &&
      std::strcmp(support_dex, app_dex) != 0) {
    jclass base_loader = env->FindClass("dalvik/system/BaseDexClassLoader");
    jmethodID add_dex_path =
        base_loader == nullptr
            ? nullptr
            : env->GetMethodID(base_loader, "addDexPath",
                               "(Ljava/lang/String;Z)V");
    jstring support_path = env->NewStringUTF(support_dex);
    if (add_dex_path == nullptr || support_path == nullptr ||
        env->ExceptionCheck()) {
      env->ExceptionClear();
      env->DeleteLocalRef(support_path);
      env->DeleteLocalRef(base_loader);
      std::cerr << "ART Darwin DEX: support addDexPath lookup failed\n";
      return 4;
    }
    // Secondary app code is untrusted application code.  Passing true asks
    // DexFile.setTrusted() for a debuggable process and is rejected by AOSP;
    // app_process uses the default untrusted path here.
    env->CallVoidMethod(managed_loader, add_dex_path, support_path, JNI_FALSE);
    const bool added = !env->ExceptionCheck();
    if (!added) {
      env->ExceptionClear();
    }
    env->DeleteLocalRef(support_path);
    env->DeleteLocalRef(base_loader);
    if (!added) {
      std::cerr << "ART Darwin DEX: support addDexPath failed\n";
      return 4;
    }
  }

  auto find = [&](const char* descriptor) -> jclass {
    art::Handle<art::mirror::Class> klass = hs.NewHandle(
        class_linker->FindClass(self, descriptor, std::strlen(descriptor),
                                app_loader));
    if (klass == nullptr || self->IsExceptionPending()) {
      return nullptr;
    }
    return soa.AddLocalReference<jclass>(klass.Get());
  };

  out->hello = find("Ldev/darwinart/probe/Hello;");
  if (out->hello == nullptr) {
    std::cerr << "ART Darwin DEX: Hello class lookup failed\n";
    return 5;
  }
  out->activity = find(activity_descriptor);
  if (out->activity == nullptr) {
    std::cerr << "ART Android framework: Activity subclass lookup failed\n";
    return 21;
  }
  out->context = find("Ldev/darwinart/probe/ProbeContext;");
  out->resources = find("Ldev/darwinart/probe/ProbeResources;");
  out->view = find("Ldev/darwinart/probe/ProbeView;");
  out->content_root = find("Ldev/darwinart/probe/ProbeContentRoot;");
  out->package_manager = find("Ldev/darwinart/probe/ProbePackageManager;");
  out->upstream_test_harness = find("Ldev/darwinart/probe/UpstreamTestHarness;");
  if (out->context == nullptr || out->resources == nullptr || out->view == nullptr ||
      out->content_root == nullptr || out->package_manager == nullptr ||
      out->upstream_test_harness == nullptr) {
    std::cerr << "ART Android window: framework probe class lookup failed\n";
    return 21;
  }

  if (headless_fixture) {
    out->canvas = find("Ldev/darwinart/probe/ProbeCanvas;");
    if (out->canvas == nullptr) {
      std::cerr << "ART Android window: ProbeCanvas lookup failed\n";
      return 21;
    }
  }
  if (run_direct_apk) {
    if (run_elf_jni_fixture) {
      std::cerr << "ART Android direct APK must run in its isolated host flavor\n";
      return 46;
    }
    std::string direct_error;
    bool direct_loaded = false;
    {
      art::ScopedThreadSuspension suspended(self, art::ThreadState::kNative);
      direct_loaded = art::Runtime::Current()->GetJavaVM()->LoadNativeLibrary(
          env, direct_apk_path, out->app_loader, nullptr, &direct_error);
    }
    if (!direct_loaded || !direct_error.empty() || env->ExceptionCheck()) {
      std::cerr << "ART Android direct APK JavaVMExt load failed, load_error="
                << direct_error << "\n";
      return 46;
    }
    darwin_art_acceptance::record_direct_apk_loaded();
  }
  if (run_elf_jni_fixture) {
    out->native_fixture = find("Ldarwin/art/nativefixture/NativeFixture;");
  }
  if (run_network_acceptance) {
    out->network_fixture =
        find("Ldev/darwinart/probe/NetworkRuntimeFixture;");
  }
  if ((run_elf_jni_fixture && out->native_fixture == nullptr) ||
      (run_network_acceptance && out->network_fixture == nullptr)) {
    std::cerr << "ART Android window: fixture class lookup failed\n";
    return 21;
  }
  return 0;
}

int load_native_library(JNIEnv* env, art::Thread* self, jobject app_loader,
                        const char* native_library_path) {
  if (env == nullptr || self == nullptr || app_loader == nullptr ||
      native_library_path == nullptr || native_library_path[0] == '\0') {
    return 46;
  }
  std::string native_error;
  bool loaded = false;
  {
    // ART's JavaVMExt loader may enter NativeBridge and execute JNI_OnLoad;
    // keep the owner thread in the native state exactly as the platform
    // Runtime.nativeLoad path does, but invoke it only after the app
    // ClassLoader has been installed by the registration phase.
    art::ScopedThreadSuspension suspended(self, art::ThreadState::kNative);
    loaded = art::Runtime::Current()->GetJavaVM()->LoadNativeLibrary(
        env, native_library_path, app_loader, nullptr, &native_error);
  }
  if (!loaded || !native_error.empty() || env->ExceptionCheck()) {
    std::cerr << "ART Android APK JNI: JavaVMExt load failed path="
              << native_library_path << " error=" << native_error << "\n";
    if (self->IsExceptionPending()) {
      std::cerr << self->GetException()->Dump() << "\n";
    }
    return 46;
  }
  std::cerr << "ART Android APK JNI: JavaVMExt+NativeBridge load ok\n";
  return 0;
}

int install_native_library_path(JNIEnv* env, jobject app_loader,
                                const char* native_library_path) {
  if (env == nullptr || app_loader == nullptr || native_library_path == nullptr ||
      native_library_path[0] == '\0') {
    return 46;
  }
  const std::string path(native_library_path);
  const std::string::size_type slash = path.find_last_of('/');
  if (slash == std::string::npos || slash == 0) {
    return 46;
  }
  const std::string directory = path.substr(0, slash);
  jclass array_list_class = env->FindClass("java/util/ArrayList");
  jclass loader_class = env->GetObjectClass(app_loader);
  if (array_list_class == nullptr || loader_class == nullptr) {
    return 46;
  }
  jmethodID array_list_ctor =
      env->GetMethodID(array_list_class, "<init>", "()V");
  jmethodID add = env->GetMethodID(array_list_class, "add", "(Ljava/lang/Object;)Z");
  jmethodID add_native_path = env->GetMethodID(
      loader_class, "addNativePath", "(Ljava/util/Collection;)V");
  jobject paths = array_list_ctor == nullptr
                      ? nullptr
                      : env->NewObject(array_list_class, array_list_ctor);
  jstring directory_string = env->NewStringUTF(directory.c_str());
  if (paths == nullptr || directory_string == nullptr || add == nullptr ||
      add_native_path == nullptr) {
    return 46;
  }
  env->CallBooleanMethod(paths, add, directory_string);
  env->CallVoidMethod(app_loader, add_native_path, paths);
  // CreatePathClassLoader starts with an empty native path element array and
  // Android's addNativePath() is intentionally best-effort on that detached
  // test loader.  Unity resolves its IL2CPP image through
  // ClassLoader.findLibrary(), which walks this array rather than the
  // ApplicationInfo field.  Materialize the same directory element that a
  // production BaseDexClassLoader constructor would install.
  if (!env->ExceptionCheck()) {
    jclass element_class =
        env->FindClass("dalvik/system/DexPathList$NativeLibraryElement");
    jclass file_class = env->FindClass("java/io/File");
    jclass base_loader_class = env->FindClass("dalvik/system/BaseDexClassLoader");
    jclass dex_path_list_class = env->FindClass("dalvik/system/DexPathList");
    jmethodID file_ctor =
        file_class == nullptr
            ? nullptr
            : env->GetMethodID(file_class, "<init>", "(Ljava/lang/String;)V");
    jmethodID element_ctor =
        element_class == nullptr
            ? nullptr
            : env->GetMethodID(element_class, "<init>", "(Ljava/io/File;)V");
    jfieldID path_list_field =
        base_loader_class == nullptr
            ? nullptr
            : env->GetFieldID(base_loader_class, "pathList",
                              "Ldalvik/system/DexPathList;");
    jfieldID elements_field =
        dex_path_list_class == nullptr
            ? nullptr
            : env->GetFieldID(
                  dex_path_list_class, "nativeLibraryPathElements",
                  "[Ldalvik/system/DexPathList$NativeLibraryElement;");
    jobject directory_file =
        file_ctor == nullptr
            ? nullptr
            : env->NewObject(file_class, file_ctor, directory_string);
    jobject element =
        element_ctor == nullptr || directory_file == nullptr
            ? nullptr
            : env->NewObject(element_class, element_ctor, directory_file);
    jobject path_list =
        path_list_field == nullptr
            ? nullptr
            : env->GetObjectField(app_loader, path_list_field);
    jobjectArray element_array =
        element_class == nullptr || element == nullptr
            ? nullptr
            : env->NewObjectArray(1, element_class, element);
    if (path_list != nullptr && elements_field != nullptr &&
        element_array != nullptr && !env->ExceptionCheck()) {
      env->SetObjectField(path_list, elements_field, element_array);
    }
    env->DeleteLocalRef(element_array);
    env->DeleteLocalRef(path_list);
    env->DeleteLocalRef(element);
    env->DeleteLocalRef(directory_file);
    env->DeleteLocalRef(dex_path_list_class);
    env->DeleteLocalRef(base_loader_class);
    env->DeleteLocalRef(file_class);
    env->DeleteLocalRef(element_class);
  }
  if (std::getenv("DARWIN_ART_DEBUG_NATIVE_LIBRARY_PATH") != nullptr &&
      !env->ExceptionCheck()) {
    jclass base_loader_class =
        env->FindClass("dalvik/system/BaseDexClassLoader");
    jclass dex_path_list_class = env->FindClass("dalvik/system/DexPathList");
    jfieldID path_list_field =
        base_loader_class == nullptr
            ? nullptr
            : env->GetFieldID(base_loader_class, "pathList",
                              "Ldalvik/system/DexPathList;");
    jfieldID elements_field =
        dex_path_list_class == nullptr
            ? nullptr
            : env->GetFieldID(
                  dex_path_list_class, "nativeLibraryPathElements",
                  "[Ldalvik/system/DexPathList$NativeLibraryElement;");
    jobject path_list =
        path_list_field == nullptr
            ? nullptr
            : env->GetObjectField(app_loader, path_list_field);
    auto elements =
        path_list == nullptr || elements_field == nullptr
            ? nullptr
            : reinterpret_cast<jobjectArray>(
                  env->GetObjectField(path_list, elements_field));
    const jsize element_count =
        elements == nullptr ? -1 : env->GetArrayLength(elements);
    std::cerr << "ART Android APK: PathClassLoader native elements="
              << element_count << "\n";
    const char* debug_library_name =
        std::getenv("DARWIN_ART_DEBUG_NATIVE_LIBRARY_NAME");
    if (debug_library_name != nullptr && debug_library_name[0] != '\0') {
      jmethodID find_library = env->GetMethodID(
          loader_class, "findLibrary", "(Ljava/lang/String;)Ljava/lang/String;");
      jstring library_name = env->NewStringUTF(debug_library_name);
      auto found_path =
          find_library == nullptr || library_name == nullptr
              ? nullptr
              : reinterpret_cast<jstring>(env->CallObjectMethod(
                    app_loader, find_library, library_name));
      const char* found_utf =
          found_path == nullptr
              ? nullptr
              : env->GetStringUTFChars(found_path, nullptr);
      std::cerr << "ART Android APK: PathClassLoader findLibrary name="
                << debug_library_name << " path="
                << (found_utf == nullptr ? "<null>" : found_utf) << "\n";
      if (found_utf != nullptr) {
        env->ReleaseStringUTFChars(found_path, found_utf);
      }
      env->DeleteLocalRef(found_path);
      env->DeleteLocalRef(library_name);
    }
    env->DeleteLocalRef(elements);
    env->DeleteLocalRef(path_list);
    env->DeleteLocalRef(dex_path_list_class);
    env->DeleteLocalRef(base_loader_class);
  }
  const bool failed = env->ExceptionCheck();
  env->DeleteLocalRef(directory_string);
  env->DeleteLocalRef(paths);
  env->DeleteLocalRef(loader_class);
  env->DeleteLocalRef(array_list_class);
  if (failed) {
    return 46;
  }
  std::cerr << "ART Android APK: PathClassLoader nativeLibraryPath installed dir="
            << directory << "\n";
  return 0;
}

}  // namespace darwin_art_app
