#include <array>
#include <atomic>
#include <cstring>
#include <iostream>
#include <string>
#include <vector>
#include <chrono>
#include <thread>
#include <cstdio>
#include <dlfcn.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <mach/arm/thread_status.h>
#include <pthread.h>

#include "darwin_jni_shorty.h"
#include "darwin_unwindstack_native.h"
#include "darwin_runtime_adapters_internal.h"
#include "jni/jni_env_ext.h"

namespace android {

namespace {

using UnityBoolNative = jboolean (*)(JNIEnv*, jobject);
using UnityVoidNative = void (*)(JNIEnv*, jobject);
using UnityFocusNative = void (*)(JNIEnv*, jobject, jboolean);
using UnitySurfaceNative = void (*)(JNIEnv*, jobject, jint, jobject);
using NativeLoaderLoad = jboolean (*)(JNIEnv*, jobject, jstring);

UnityBoolNative g_unity_native_render = nullptr;
UnityBoolNative g_unity_native_pause = nullptr;
UnityVoidNative g_unity_native_resume = nullptr;
UnityFocusNative g_unity_native_focus_changed = nullptr;
UnitySurfaceNative g_unity_native_recreate_gfx_state = nullptr;
NativeLoaderLoad g_native_loader_load = nullptr;

bool UnityLifecycleDebugEnabled() {
  return std::getenv("DARWIN_ART_DEBUG_UNITY_LIFECYCLE") != nullptr;
}

jboolean TraceUnityNativeRender(JNIEnv* env, jobject object) {
  static std::atomic<bool> watched{false};
  static std::atomic<bool> finished{false};
  const bool watch = std::getenv("DARWIN_ART_DEBUG_UNITY_STALL") != nullptr &&
                     !watched.exchange(true);
  if (watch) {
    const mach_port_t target = mach_thread_self();
    const uintptr_t stack_end = reinterpret_cast<uintptr_t>(
        pthread_get_stackaddr_np(pthread_self()));
    const uintptr_t stack_begin = stack_end - pthread_get_stacksize_np(pthread_self());
    std::thread([target, stack_begin, stack_end] {
      std::this_thread::sleep_for(std::chrono::seconds(5));
      if (!finished.load(std::memory_order_acquire)) {
        const auto sample = [](mach_port_t sample_thread, uintptr_t stack_begin,
                               uintptr_t stack_end) {
        thread_extended_info_data_t thread_info_data{};
        mach_msg_type_number_t info_count = THREAD_EXTENDED_INFO_COUNT;
        (void)thread_info(sample_thread, THREAD_EXTENDED_INFO,
            reinterpret_cast<thread_info_t>(&thread_info_data), &info_count);
        // Self-process inspection avoids external debugger permissions. Keep
        // all allocation, dyld lookup, and logging outside the suspended
        // interval: the target may own those libraries' internal locks.
        arm_thread_state64_t state{};
        mach_msg_type_number_t count = ARM_THREAD_STATE64_COUNT;
        uintptr_t addresses[40]{};
        size_t depth = 0;
        std::fprintf(stderr, "DARWIN Unity stall: sampling first nativeRender\n");
        const kern_return_t suspended = thread_suspend(sample_thread);
        kern_return_t status = suspended;
        if (suspended == KERN_SUCCESS) {
          status = thread_get_state(sample_thread, ARM_THREAD_STATE64,
              reinterpret_cast<thread_state_t>(&state), &count);
          if (status == KERN_SUCCESS) {
            addresses[depth++] = arm_thread_state64_get_pc(state);
            addresses[depth++] = arm_thread_state64_get_lr(state);
            if (stack_begin == 0) {
              stack_begin = arm_thread_state64_get_sp(state);
              stack_end = stack_begin + 8 * 1024 * 1024;
            }
            uintptr_t fp = arm_thread_state64_get_fp(state);
            while (depth < 40 && fp >= stack_begin &&
                   fp <= stack_end - 2 * sizeof(uintptr_t) && (fp & 15) == 0) {
              uintptr_t frame[2]{};
              mach_vm_size_t copied = 0;
              if (mach_vm_read_overwrite(mach_task_self(), fp, sizeof(frame),
                      reinterpret_cast<mach_vm_address_t>(frame), &copied) != KERN_SUCCESS ||
                  copied != sizeof(frame)) break;
              addresses[depth++] = frame[1] & 0x0000ffffffffffffULL;
              if (frame[0] <= fp) break;
              fp = frame[0];
            }
          }
          (void)thread_resume(sample_thread);
        }
        std::fprintf(stderr, "DARWIN Unity stall: thread=%u name=%s status=%d pc=%p fp=%p sp=%p frames=%zu x19=%llx x20=%llx x21=%llx\n",
            sample_thread, thread_info_data.pth_name, status,
            reinterpret_cast<void*>(arm_thread_state64_get_pc(state)),
            reinterpret_cast<void*>(arm_thread_state64_get_fp(state)),
            reinterpret_cast<void*>(arm_thread_state64_get_sp(state)), depth,
            state.__x[19], state.__x[20], state.__x[21]);
        for (size_t i = 0; i < depth; ++i) {
          Dl_info info{};
          const bool resolved = dladdr(reinterpret_cast<void*>(addresses[i]), &info) != 0;
          std::fprintf(stderr, "DARWIN Unity stall frame=%zu pc=%p image=%s symbol=%s offset=0x%llx\n",
              i, reinterpret_cast<void*>(addresses[i]),
              resolved && info.dli_fname ? info.dli_fname : "guest-elf",
              resolved && info.dli_sname ? info.dli_sname : "unknown",
              static_cast<unsigned long long>(addresses[i] -
                  reinterpret_cast<uintptr_t>(resolved ? info.dli_fbase : nullptr)));
        }
        };
        sample(target, stack_begin, stack_end);
        if (std::getenv("DARWIN_ART_DEBUG_UNITY_STALL_ALL") != nullptr) {
          thread_act_array_t threads = nullptr;
          mach_msg_type_number_t thread_count = 0;
          const mach_port_t watcher = mach_thread_self();
          if (task_threads(mach_task_self(), &threads, &thread_count) == KERN_SUCCESS) {
            for (mach_msg_type_number_t i = 0; i < thread_count; ++i) {
              if (threads[i] != watcher && threads[i] != target) sample(threads[i], 0, 0);
              mach_port_deallocate(mach_task_self(), threads[i]);
            }
            vm_deallocate(mach_task_self(), reinterpret_cast<vm_address_t>(threads),
                          thread_count * sizeof(thread_t));
          }
          mach_port_deallocate(mach_task_self(), watcher);
        }
      }
      mach_port_deallocate(mach_task_self(), target);
    }).detach();
  }
  std::cerr << "DARWIN Unity lifecycle: nativeRender enter\n";
  const jboolean result = g_unity_native_render == nullptr
                              ? JNI_FALSE
                              : g_unity_native_render(env, object);
  if (watch) finished.store(true, std::memory_order_release);
  std::cerr << "DARWIN Unity lifecycle: nativeRender exit result="
            << (result == JNI_TRUE ? 1 : 0) << " exception="
            << (env != nullptr && env->ExceptionCheck() ? 1 : 0) << "\n";
  return result;
}

jboolean TraceUnityNativePause(JNIEnv* env, jobject object) {
  std::cerr << "DARWIN Unity lifecycle: nativePause enter\n";
  const jboolean result = g_unity_native_pause == nullptr
                              ? JNI_FALSE
                              : g_unity_native_pause(env, object);
  std::cerr << "DARWIN Unity lifecycle: nativePause exit result="
            << (result == JNI_TRUE ? 1 : 0) << " exception="
            << (env != nullptr && env->ExceptionCheck() ? 1 : 0) << "\n";
  return result;
}

void TraceUnityNativeResume(JNIEnv* env, jobject object) {
  std::cerr << "DARWIN Unity lifecycle: nativeResume enter\n";
  if (g_unity_native_resume != nullptr) g_unity_native_resume(env, object);
  std::cerr << "DARWIN Unity lifecycle: nativeResume exit exception="
            << (env != nullptr && env->ExceptionCheck() ? 1 : 0) << "\n";
}

void TraceUnityNativeFocusChanged(JNIEnv* env, jobject object, jboolean focused) {
  std::cerr << "DARWIN Unity lifecycle: nativeFocusChanged enter focused="
            << (focused == JNI_TRUE ? 1 : 0) << "\n";
  if (g_unity_native_focus_changed != nullptr)
    g_unity_native_focus_changed(env, object, focused);
  std::cerr << "DARWIN Unity lifecycle: nativeFocusChanged exit exception="
            << (env != nullptr && env->ExceptionCheck() ? 1 : 0) << "\n";
}

void TraceUnityNativeRecreateGfxState(JNIEnv* env, jobject object, jint index,
                                      jobject surface) {
  std::cerr << "DARWIN Unity lifecycle: nativeRecreateGfxState enter index="
            << index << " surface=" << surface << "\n";
  if (g_unity_native_recreate_gfx_state != nullptr)
    g_unity_native_recreate_gfx_state(env, object, index, surface);
  std::cerr << "DARWIN Unity lifecycle: nativeRecreateGfxState exit exception="
            << (env != nullptr && env->ExceptionCheck() ? 1 : 0) << "\n";
}

jboolean TraceNativeLoaderLoad(JNIEnv* env, jobject object, jstring path) {
  std::cerr << "DARWIN NativeLoader.load enter path=" << path << "\n";
  const jboolean result = g_native_loader_load == nullptr
                              ? JNI_FALSE
                              : g_native_loader_load(env, object, path);
  std::cerr << "DARWIN NativeLoader.load exit result="
            << (result == JNI_TRUE ? 1 : 0) << " exception="
            << (env != nullptr && env->ExceptionCheck() ? 1 : 0) << "\n";
  return result;
}

}  // namespace

void* MaybeWrapUnityLifecycleNative(const char* name, const char* signature,
                                    void* target) {
  if (!UnityLifecycleDebugEnabled() || name == nullptr || signature == nullptr ||
      target == nullptr)
    return target;
  if (std::strcmp(name, "nativeRender") == 0 &&
      std::strcmp(signature, "()Z") == 0) {
    g_unity_native_render = reinterpret_cast<UnityBoolNative>(target);
    return reinterpret_cast<void*>(&TraceUnityNativeRender);
  }
  if (std::strcmp(name, "nativePause") == 0 &&
      std::strcmp(signature, "()Z") == 0) {
    g_unity_native_pause = reinterpret_cast<UnityBoolNative>(target);
    return reinterpret_cast<void*>(&TraceUnityNativePause);
  }
  if (std::strcmp(name, "nativeResume") == 0 &&
      std::strcmp(signature, "()V") == 0) {
    g_unity_native_resume = reinterpret_cast<UnityVoidNative>(target);
    return reinterpret_cast<void*>(&TraceUnityNativeResume);
  }
  if (std::strcmp(name, "nativeFocusChanged") == 0 &&
      std::strcmp(signature, "(Z)V") == 0) {
    g_unity_native_focus_changed = reinterpret_cast<UnityFocusNative>(target);
    return reinterpret_cast<void*>(&TraceUnityNativeFocusChanged);
  }
  if (std::strcmp(name, "nativeRecreateGfxState") == 0 &&
      std::strcmp(signature, "(ILandroid/view/Surface;)V") == 0) {
    g_unity_native_recreate_gfx_state =
        reinterpret_cast<UnitySurfaceNative>(target);
    return reinterpret_cast<void*>(&TraceUnityNativeRecreateGfxState);
  }
  if (std::strcmp(name, "load") == 0 &&
      std::strcmp(signature, "(Ljava/lang/String;)Z") == 0) {
    g_native_loader_load = reinterpret_cast<NativeLoaderLoad>(target);
    return reinterpret_cast<void*>(&TraceNativeLoaderLoad);
  }
  return target;
}


int32_t ProxyRegisterNatives(void* context,
                             void* clazz,
                             const DarwinArtJniNativeMethod* methods,
                             int32_t count) {
  auto* library = static_cast<ElfLibrary*>(context);
  constexpr int32_t kMaxRegularMethodsPerGraph = 4096;
  if (library == nullptr || clazz == nullptr || methods == nullptr ||
      count <= 0 || count > kMaxRegularMethodsPerGraph) {
    std::cerr << "DARWIN JNI RegisterNatives reject: bad arguments count=" << count << "\n";
    return DARWIN_ART_JNI_ERR;
  }
  // NativeLoader's guest libdl facade is process-wide, while each Android DSO
  // still owns an independent ELF image registry and JNI trampoline set.  A
  // child DSO can therefore enter this callback through the first DSO's proxy
  // context.  Resolve the actual owner from the first native method address;
  // this is the same image-ownership fact Android's linker uses for the
  // registration and keeps the validation fail-closed for foreign pointers.
  if (methods[0].function != nullptr) {
    if (ElfLibrary* image_owner = FindElfLibraryForAddress(
            reinterpret_cast<uintptr_t>(methods[0].function));
        image_owner != nullptr) {
      library = image_owner;
    }
  }
  JNIEnv* art_env = CurrentArtEnv();
  if (library->proxy == nullptr || art_env == nullptr) {
    std::cerr << "DARWIN JNI RegisterNatives reject: proxy/env missing proxy="
              << library->proxy << " env=" << art_env << "\n";
    return DARWIN_ART_JNI_ERR;
  }
  for (size_t index = 0; index < static_cast<size_t>(count); ++index) {
    const bool image_contains = darwin_art_image_registry::ContainsAddress(
        library->image_registry,
        reinterpret_cast<uintptr_t>(methods[index].function));
    if (methods[index].name == nullptr || methods[index].signature == nullptr ||
        methods[index].function == nullptr ||
        !image_contains) {
      return DARWIN_ART_JNI_ERR;
    }
    jmethodID method = art_env->GetStaticMethodID(
        static_cast<jclass>(clazz), methods[index].name, methods[index].signature);
    if (method == nullptr && art_env->ExceptionCheck()) {
      art_env->ExceptionClear();
    }
    // JNI_OnLoad commonly registers a static utility class followed by an
    // instance class.  The old bridge only checked GetStaticMethodID, which
    // made the second, perfectly valid RegisterNatives call fail closed.
    if (method == nullptr) {
      method = art_env->GetMethodID(static_cast<jclass>(clazz), methods[index].name,
                                    methods[index].signature);
      if (method == nullptr && art_env->ExceptionCheck()) {
        art_env->ExceptionClear();
      }
    }
    if (method == nullptr) {
      std::cerr << "DARWIN JNI RegisterNatives reject: method missing name="
                << methods[index].name << " sig=" << methods[index].signature << "\n";
      return DARWIN_ART_JNI_ERR;
    }
  }
  JavaVM* proxy_vm =
      static_cast<JavaVM*>(darwin_art_jni_proxy_java_vm(library->proxy));
  void* proxy_env = nullptr;
  if (proxy_vm == nullptr || proxy_vm->GetEnv(&proxy_env, JNI_VERSION_1_6) != JNI_OK ||
      proxy_env == nullptr) {
    return DARWIN_ART_JNI_ERR;
  }
  std::vector<std::string> shorties(static_cast<size_t>(count));
  std::vector<darwin_art::android_jni::TrampolineRequest> requests(
      static_cast<size_t>(count));
  for (size_t index = 0; index < requests.size(); ++index) {
    if (!DescriptorToShorty(methods[index].signature, &shorties[index])) {
      return DARWIN_ART_JNI_ERR;
    }
    requests[index] = {
        MaybeWrapUnityLifecycleNative(methods[index].name,
                                      methods[index].signature,
                                      methods[index].function),
        shorties[index].c_str(), uint32_t{1}};
    std::cerr << "DARWIN JNI RegisterNatives method name="
              << methods[index].name << " sig=" << methods[index].signature
              << " target=" << methods[index].function << "\n";
  }
  std::string trampoline_error;
  auto* trampolines = darwin_art::android_jni::CreateRegularTrampolines(
      proxy_env, requests.data(), requests.size(), &trampoline_error);
  if (trampolines == nullptr) {
    std::cerr << "DARWIN JNI RegisterNatives reject: trampoline create " << trampoline_error
              << "\n";
    return DARWIN_ART_JNI_ERR;
  }
  bool all_entries_valid = true;
  for (size_t index = 0; index < requests.size(); ++index) {
    void* entry = darwin_art::android_jni::TrampolineEntry(trampolines, index);
    all_entries_valid = all_entries_valid && entry != nullptr &&
                        darwin_art::android_jni::TrampolineEntryMask(entry) ==
                            requests[index].entry_mask;
  }
  if (darwin_art::android_jni::TrampolineGeneration(trampolines) == 0 ||
      darwin_art::android_jni::TrampolineCount(trampolines) != requests.size() ||
      !all_entries_valid) {
    darwin_art::android_jni::DestroyRegularTrampolines(trampolines);
    return DARWIN_ART_JNI_ERR;
  }
  std::vector<JNINativeMethod> bridged_methods(static_cast<size_t>(count));
  for (size_t index = 0; index < bridged_methods.size(); ++index) {
    bridged_methods[index] = {
        const_cast<char*>(methods[index].name), const_cast<char*>(methods[index].signature),
        darwin_art::android_jni::TrampolineEntry(trampolines, index)};
  }
  const jint status = art_env->RegisterNatives(
      static_cast<jclass>(clazz), bridged_methods.data(),
      static_cast<jint>(bridged_methods.size()));
  if (status != JNI_OK) {
    std::cerr << "DARWIN JNI RegisterNatives reject: ART status=" << status
              << " class=" << clazz << " count=" << count << "\n";
    jthrowable registration_failure = art_env->ExceptionOccurred();
    if (registration_failure != nullptr) art_env->ExceptionClear();
    art_env->UnregisterNatives(static_cast<jclass>(clazz));
    jthrowable rollback_failure = art_env->ExceptionOccurred();
    if (rollback_failure != nullptr) art_env->ExceptionClear();
    darwin_art::android_jni::DestroyRegularTrampolines(trampolines);
    if (registration_failure != nullptr) {
      art_env->Throw(registration_failure);
      art_env->DeleteLocalRef(registration_failure);
    } else if (rollback_failure != nullptr) {
      art_env->Throw(rollback_failure);
    }
    art_env->DeleteLocalRef(rollback_failure);
    return DARWIN_ART_JNI_ERR;
  }
  // RegisterNatives above is the point where ART has installed each host
  // trampoline into the ArtMethod. Preserve both sides of that binding for
  // unwindstack: the trampoline is what appears in a stopped ART thread,
  // while the target remains the guest ELF address used for symbol lookup.
  for (size_t index = 0; index < requests.size(); ++index) {
    darwin_art_register_native_entry_pair(
        reinterpret_cast<uintptr_t>(darwin_art::android_jni::TrampolineEntry(
            trampolines, index)),
        reinterpret_cast<uintptr_t>(methods[index].function), methods[index].name);
  }
  {
    std::lock_guard<std::mutex> lock(library->trampoline_mutex);
    if (library->trampolines == nullptr) {
      library->trampolines = trampolines;
    }
    library->trampoline_sets.push_back(trampolines);
  }
  std::cerr << "DARWIN JNI RegisterNatives installed count=" << count
            << " sets=" << library->trampoline_sets.size() << "\n";
  return DARWIN_ART_JNI_OK;
}

}  // namespace android
