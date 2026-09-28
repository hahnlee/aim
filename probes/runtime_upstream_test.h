#pragma once

#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <initializer_list>
#include <unistd.h>
#include <vector>

#include "instrumentation.h"
#include "jit/jit.h"

namespace aim_upstream_test {

inline bool ResolveMainDexStrings(JNIEnv* env,
                                  art::ScopedObjectAccess& soa,
                                  art::StackHandleScope<32>& hs,
                                  jclass harness) {
  const char* target = std::getenv("AIM_UPSTREAM_MAIN");
  jmethodID load = env->GetStaticMethodID(
      harness, "load", "(Ljava/lang/String;)Ljava/lang/Class;");
  jstring class_name = target == nullptr ? nullptr : env->NewStringUTF(target);
  jclass target_class = load == nullptr || class_name == nullptr
      ? nullptr
      : static_cast<jclass>(env->CallStaticObjectMethod(harness, load, class_name));
  if (target_class == nullptr || env->ExceptionCheck()) return false;
  art::Handle<art::mirror::Class> target_handle =
      hs.NewHandle(soa.Decode<art::mirror::Class>(target_class));
  art::Handle<art::mirror::DexCache> dex_cache =
      hs.NewHandle(target_handle->GetDexCache());
  const art::DexFile* dex_file = dex_cache->GetDexFile();
  art::ClassLinker* linker = art::Runtime::Current()->GetClassLinker();
  for (uint32_t index = 0; index < dex_file->NumStringIds(); ++index) {
    if (linker->ResolveString(art::dex::StringIndex(index), dex_cache) == nullptr ||
        env->ExceptionCheck()) {
      return false;
    }
  }
  env->DeleteLocalRef(target_class);
  env->DeleteLocalRef(class_name);
  return true;
}

inline bool CompileMain(JNIEnv* env,
                        art::Thread* self,
                        art::ScopedObjectAccess& soa,
                        art::StackHandleScope<32>& hs,
                        jclass harness) {
  const char* target = std::getenv("AIM_UPSTREAM_MAIN");
  jmethodID load = env->GetStaticMethodID(
      harness, "load", "(Ljava/lang/String;)Ljava/lang/Class;");
  jstring class_name = target == nullptr ? nullptr : env->NewStringUTF(target);
  jclass target_class = load == nullptr || class_name == nullptr
      ? nullptr
      : static_cast<jclass>(env->CallStaticObjectMethod(harness, load, class_name));
  if (target_class == nullptr || env->ExceptionCheck()) return false;
  art::Handle<art::mirror::Class> target_handle =
      hs.NewHandle(soa.Decode<art::mirror::Class>(target_class));
  art::ClassLinker* linker = art::Runtime::Current()->GetClassLinker();
  if (!linker->EnsureInitialized(self, target_handle, true, true)) return false;
  linker->MakeInitializedClassesVisiblyInitialized(self, /*wait=*/ true);
  art::ArtMethod* main = target_handle->FindClassMethod(
      "main", "([Ljava/lang/String;)V", art::kRuntimePointerSize);
  art::jit::Jit* jit = art::Runtime::Current()->GetJit();
  art::ArtMethod* installed_method = nullptr;
  bool selected_aot = false;
  // The optimized differential lane must exercise the live JIT entrypoint;
  // AOT selection is retained only for explicit launcher requests that model
  // a production precompiled app process.
  const bool prefer_aot =
      std::getenv("AIM_UPSTREAM_PREFER_AOT") != nullptr;
  if (prefer_aot && main != nullptr) {
    const void* oat_code =
        main->GetOatMethodQuickCode(art::kRuntimePointerSize);
    if (oat_code != nullptr &&
        art::Runtime::Current()->GetInstrumentation()->GetCodeForInvoke(main) ==
            oat_code) {
      installed_method = main;
      selected_aot = true;
    }
  }
  // Android does not replace an already selected speed-compiled AOT entrypoint
  // merely because JIT is enabled for the process. Compile an application
  // method explicitly only for the differential harness's no-AOT path.
  if (installed_method == nullptr && main != nullptr && jit != nullptr &&
      jit->CompileMethod(main, self, art::CompilationKind::kOptimized, false) &&
      jit->GetCodeCache()->ContainsPc(main->GetEntryPointFromQuickCompiledCode())) {
    installed_method = main;
  }

  const char* upstream_test = std::getenv("AIM_UPSTREAM_TEST_NAME");
  if (jit != nullptr && upstream_test != nullptr &&
      std::strcmp(upstream_test, "137-cfi") == 0) {
    auto compile_named = [&](art::ObjPtr<art::mirror::Class> klass,
                             std::initializer_list<std::string_view> names) {
      for (art::ArtMethod& method :
           klass->GetDeclaredMethodsSlice(art::kRuntimePointerSize)) {
        if (method.IsNative() || method.IsAbstract() ||
            std::find(names.begin(), names.end(), method.GetNameView()) ==
                names.end()) {
          continue;
        }
        jit->CompileMethod(&method, self, art::CompilationKind::kOptimized,
                           false);
      }
    };
    compile_named(target_handle.Get(), {"main", "test", "compare", "unwind"});
    compile_named(target_handle->GetSuperClass(), {"$noinline$runTest"});
    jclass arrays_class = env->FindClass("java/util/Arrays");
    if (arrays_class != nullptr && !env->ExceptionCheck()) {
      compile_named(soa.Decode<art::mirror::Class>(arrays_class),
                    {"binarySearch", "binarySearch0"});
      env->DeleteLocalRef(arrays_class);
    }
  }

  // Generated ART tests can intentionally make Main.main too large for the
  // optimizing compiler. Android leaves such dispatcher methods interpreted;
  // their small application methods are still normal JIT candidates. Preserve
  // that policy and prove optimized execution by installing a method that the
  // unmodified dispatcher will call, rather than weakening the JIT gate.
  if (installed_method == nullptr && jit != nullptr) {
    for (art::ArtMethod& method :
         target_handle->GetDeclaredMethodsSlice(art::kRuntimePointerSize)) {
      const std::string_view name = method.GetNameView();
      if (&method == main || name == "<clinit>" ||
          method.IsNative() || method.IsAbstract() || !method.IsCompilable() ||
          method.GetCodeItem() == nullptr) {
        continue;
      }
      if (jit->CompileMethod(
              &method, self, art::CompilationKind::kOptimized, false) &&
          jit->GetCodeCache()->ContainsPc(
              method.GetEntryPointFromQuickCompiledCode())) {
        installed_method = &method;
        break;
      }
    }
  }
  env->DeleteLocalRef(target_class);
  env->DeleteLocalRef(class_name);
  if (installed_method != nullptr) {
    std::cerr << "ART upstream test: "
              << (selected_aot ? "AOT application method selected "
                               : "optimized application method installed ")
              << installed_method->PrettyMethod() << "\n";
  }
  return installed_method != nullptr;
}

inline const char* OutputPath(jint channel) {
  return std::getenv(channel == 0 ? "AIM_UPSTREAM_STDOUT"
                                  : "AIM_UPSTREAM_STDERR");
}

inline bool InitializeOutput(jint channel) {
  const char* path = OutputPath(channel);
  if (path == nullptr || path[0] == '\0') return false;
  if (channel == 0 &&
      std::getenv("AIM_UPSTREAM_INHERIT_STDOUT") != nullptr) {
    // Startup agents can write ONLOAD/VMStart bytes before Java's System.out
    // bridge is installed. The parent created this file already; preserve it.
    return true;
  }
  std::ofstream output(path, std::ios::binary | std::ios::trunc);
  output.close();
  return output.good();
}

inline void WriteOutputByte(JNIEnv*, jclass, jint channel, jint value) {
  const char byte = static_cast<char>(value);
  if (channel == 0 &&
      std::getenv("AIM_UPSTREAM_INHERIT_STDOUT") != nullptr) {
    (void)::write(STDOUT_FILENO, &byte, 1);
    return;
  }
  const char* path = OutputPath(channel);
  if (path == nullptr) return;
  std::ofstream output(path, std::ios::binary | std::ios::app);
  output.write(&byte, 1);
}

inline void WriteOutputChunk(JNIEnv* env,
                             jclass,
                             jint channel,
                             jbyteArray bytes,
                             jint offset,
                             jint length) {
  const char* path = OutputPath(channel);
  if (path == nullptr || bytes == nullptr || offset < 0 || length < 0 ||
      length > env->GetArrayLength(bytes) - offset) return;
  std::vector<jbyte> data(static_cast<size_t>(length));
  env->GetByteArrayRegion(bytes, offset, length, data.data());
  if (env->ExceptionCheck()) return;
  if (channel == 0 &&
      std::getenv("AIM_UPSTREAM_INHERIT_STDOUT") != nullptr) {
    const char* cursor = reinterpret_cast<const char*>(data.data());
    size_t remaining = static_cast<size_t>(length);
    while (remaining != 0) {
      const ssize_t written = ::write(STDOUT_FILENO, cursor, remaining);
      if (written <= 0) return;
      cursor += written;
      remaining -= static_cast<size_t>(written);
    }
    return;
  }
  std::ofstream output(path, std::ios::binary | std::ios::app);
  output.write(reinterpret_cast<const char*>(data.data()), length);
}

inline bool OutputExists(const char* path) {
  if (path == nullptr || path[0] == '\0') return false;
  std::ifstream input(path, std::ios::binary);
  return input.good();
}

class ScopedNativeStderrCapture {
 public:
  explicit ScopedNativeStderrCapture(const char* path)
      : output_(path == nullptr ? "" : path,
                std::ios::binary | std::ios::app),
        previous_(output_.good() ? std::cerr.rdbuf(output_.rdbuf()) : nullptr) {}

  ~ScopedNativeStderrCapture() {
    if (previous_ != nullptr) {
      std::cerr.flush();
      std::cerr.rdbuf(previous_);
    }
  }

  bool IsValid() const { return previous_ != nullptr; }

  ScopedNativeStderrCapture(const ScopedNativeStderrCapture&) = delete;
  ScopedNativeStderrCapture& operator=(const ScopedNativeStderrCapture&) = delete;

 private:
  std::ofstream output_;
  std::streambuf* previous_;
};

inline bool Prepare(JNIEnv* env, jclass harness) {
  if (!InitializeOutput(0) || !InitializeOutput(1)) return false;
  // Bionic's run-test process stream preserves native printf ordering with
  // Java's descriptor writes. Darwin switches stdout to block buffering when
  // the runner redirects it to a file, which otherwise delays short JNI
  // lines until process exit and reorders the observable Android stream.
  if (std::getenv("AIM_UPSTREAM_INHERIT_STDOUT") != nullptr &&
      std::setvbuf(stdout, nullptr, _IONBF, 0) != 0) {
    return false;
  }
  JNINativeMethod output_natives[] = {
    {
      const_cast<char*>("writeOutputByte"),
      const_cast<char*>("(II)V"),
      reinterpret_cast<void*>(&WriteOutputByte),
    },
    {
      const_cast<char*>("writeOutputChunk"),
      const_cast<char*>("(I[BII)V"),
      reinterpret_cast<void*>(&WriteOutputChunk),
    },
  };
  if (env->RegisterNatives(harness, output_natives, std::size(output_natives)) != JNI_OK) {
    return false;
  }
  jmethodID prepare = env->GetStaticMethodID(harness, "prepareOutput", "()V");
  if (prepare == nullptr || env->ExceptionCheck()) return false;
  env->CallStaticVoidMethod(harness, prepare);
  return !env->ExceptionCheck();
}

inline int Run(JNIEnv* env, jclass harness) {
  const char* target = std::getenv("AIM_UPSTREAM_MAIN");
  if (target == nullptr || target[0] == '\0' || harness == nullptr) return 120;
  // dalvikvm enters the application class directly. Calling through the Java
  // harness would leave UpstreamTestHarness.run/Method.invoke on the active
  // stack, and ART method tracing would incorrectly serialize those frames
  // into the application's main trace.
  jmethodID load_main_class = env->GetStaticMethodID(
      harness, "load", "(Ljava/lang/String;)Ljava/lang/Class;");
  jstring class_name = env->NewStringUTF(target);
  jclass string_class = env->FindClass("java/lang/String");
  std::vector<const char*> upstream_arguments;
  for (size_t index = 0; index != 64; ++index) {
    char name[64];
    std::snprintf(name, sizeof(name), "AIM_UPSTREAM_ARG%zu", index);
    const char* value = std::getenv(name);
    if (value == nullptr) break;
    upstream_arguments.push_back(value);
  }
  const jsize argument_count = static_cast<jsize>(upstream_arguments.size());
  jobjectArray arguments = string_class == nullptr
      ? nullptr
      : env->NewObjectArray(argument_count, string_class, nullptr);
  for (jsize index = 0; arguments != nullptr && index != argument_count; ++index) {
    jstring value = env->NewStringUTF(upstream_arguments[index]);
    if (value != nullptr) env->SetObjectArrayElement(arguments, index, value);
    env->DeleteLocalRef(value);
  }
  if (class_name == nullptr || arguments == nullptr || env->ExceptionCheck()) return 120;
  jclass main_class = load_main_class == nullptr || class_name == nullptr
      ? nullptr
      : static_cast<jclass>(env->CallStaticObjectMethod(
            harness, load_main_class, class_name));
  jmethodID main_method = main_class == nullptr || env->ExceptionCheck()
      ? nullptr
      : env->GetStaticMethodID(main_class, "main", "([Ljava/lang/String;)V");
  if (main_method == nullptr || env->ExceptionCheck()) {
    env->ExceptionClear();
    env->DeleteLocalRef(main_class);
    env->DeleteLocalRef(arguments);
    env->DeleteLocalRef(string_class);
    env->DeleteLocalRef(class_name);
    return 120;
  }
  {
    ScopedNativeStderrCapture native_stderr(OutputPath(1));
    if (!native_stderr.IsValid()) return 120;
    env->CallStaticVoidMethod(main_class, main_method, arguments);
  }
  if (env->ExceptionCheck()) {
    // Match ART's dalvikvm thread teardown: dispatch the pending Throwable
    // through Thread's uncaught-exception handler.  This both preserves the
    // application's handler (for example exception2's System.exit path) and
    // gives the default handler the AOSP-compatible "Exception in thread"
    // prefix for uncaught BootstrapMethodError cases.
    art::Thread* current = art::Thread::Current();
    art::ObjPtr<art::mirror::Throwable> exception = current->GetException();
    current->ClearException();
    art::WellKnownClasses::java_lang_Thread_dispatchUncaughtException
        ->InvokeFinal<'V', 'L'>(current, current->GetPeer(), exception);
    current->ClearException();
    env->DeleteLocalRef(main_class);
    env->DeleteLocalRef(arguments);
    env->DeleteLocalRef(string_class);
    env->DeleteLocalRef(class_name);
    return 122;
  }
  bool wrote = !env->ExceptionCheck() &&
      OutputExists(std::getenv("AIM_UPSTREAM_STDOUT")) &&
      OutputExists(std::getenv("AIM_UPSTREAM_STDERR"));
  env->DeleteLocalRef(main_class);
  env->DeleteLocalRef(arguments);
  env->DeleteLocalRef(string_class);
  env->DeleteLocalRef(class_name);
  if (!wrote) return 121;
  std::cerr << "ART upstream test: " << target << " main(String[]) PASS\n";
  return 0;
}

}  // namespace aim_upstream_test
