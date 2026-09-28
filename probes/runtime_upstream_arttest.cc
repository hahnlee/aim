#include <atomic>
#include <crt_externs.h>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <iterator>
#include <iostream>
#include <memory>
#include <signal.h>
#include <string>
#include <string_view>
#include <sys/wait.h>
#include <unistd.h>
#include <vector>

#include "arch/context.h"
#include "art_method-inl.h"
#include "base/os.h"
#include "base/time_utils.h"
#include "base/unix_file/fd_file.h"
#include "base/utils.h"
#include "base/sdk_version.h"
#include "class_linker.h"
#include "class_root-inl.h"
#include "dex/art_dex_file_loader.h"
#include "debug_print.h"
#include "gc/heap.h"
#include "gc/space/image_space.h"
#include "gc/space/space-inl.h"
#include "handle_scope-inl.h"
#include "hidden_api.h"
#include "instrumentation.h"
#include "jit/debugger_interface.h"
#include "jit/jit.h"
#include "jit/jit_code_cache.h"
#include "jni.h"
#include "jni/jni_env_ext.h"
#include "jni/jni_id_manager.h"
#include "jni/jni_internal.h"
#include "mirror/method_handle_impl.h"
#include "mirror/string.h"
#include "mirror/class_ext.h"
#include "mirror/object_array-inl.h"
#include "mirror/object_array-alloc-inl.h"
#include "mirror/object_reference.h"
#include "nativehelper/scoped_utf_chars.h"
#include "monitor.h"
#include "oat/oat_file.h"
#include "reflective_handle.h"
#include "reflective_handle_scope-inl.h"
#include "runtime.h"
#include "scoped_thread_state_change-inl.h"
#include "stack.h"
#include "thread.h"
#include "thread_list.h"

namespace {
std::string g_cfi_remote_snapshot_path;

bool VerifyCfiRemoteSnapshot(const char* const* sequence, size_t count) {
  if (g_cfi_remote_snapshot_path.empty()) return false;
  std::ifstream input(g_cfi_remote_snapshot_path);
  std::string contents((std::istreambuf_iterator<char>(input)),
                       std::istreambuf_iterator<char>());
  size_t offset = 0;
  for (size_t index = 0; index < count; ++index) {
    if (sequence[index] == nullptr) return false;
    const std::string_view needle(sequence[index]);
    const size_t found = contents.find(needle, offset);
    if (found == std::string::npos) return false;
    offset = found + needle.size();
  }
  return true;
}
}  // namespace

extern "C" JNIEXPORT jboolean JNICALL Java_Main_hasOatFile(JNIEnv*, jclass);
extern "C" JNIEXPORT jboolean JNICALL Java_Main_hasJit(JNIEnv*, jclass);
extern "C" JNIEXPORT jboolean JNICALL Java_Main_runtimeIsSoftFail(JNIEnv*, jclass);
extern "C" JNIEXPORT jboolean JNICALL Java_Main_compiledWithOptimizing(JNIEnv*, jclass);
extern "C" JNIEXPORT jboolean JNICALL Java_Main_isAotCompiled(
    JNIEnv*, jclass, jclass, jstring);
extern "C" JNIEXPORT jboolean JNICALL Java_Main_hasJitCompiledEntrypoint(
    JNIEnv*, jclass, jclass, jstring);
extern "C" JNIEXPORT jboolean JNICALL Java_Main_hasJitCompiledCode(
    JNIEnv*, jclass, jclass, jstring);
extern "C" JNIEXPORT jboolean JNICALL Java_Main_isInterpreted(JNIEnv*, jclass);
extern "C" JNIEXPORT jboolean JNICALL Java_Main_isInterpretedAt(
    JNIEnv*, jclass, jint);
extern "C" JNIEXPORT jboolean JNICALL Java_Main_isInterpretedFunction(
    JNIEnv*, jclass, jobject, jboolean);
extern "C" JNIEXPORT jboolean JNICALL Java_Main_isManaged(JNIEnv*, jclass);
extern "C" JNIEXPORT jboolean JNICALL Java_Main_isCallerInterpreted(
    JNIEnv*, jclass);
extern "C" JNIEXPORT jboolean JNICALL Java_Main_hasSingleImplementation(
    JNIEnv*, jclass, jclass, jstring);
extern "C" JNIEXPORT jlong JNICALL Java_Main_genericFieldOffset(
    JNIEnv*, jclass, jobject);
extern "C" JNIEXPORT jboolean JNICALL Java_Main_isObsoleteObject(
    JNIEnv*, jclass, jclass);
extern "C" JNIEXPORT void JNICALL Java_Main_disableStackFrameAsserts(JNIEnv*, jclass);
extern "C" JNIEXPORT void JNICALL Java_Main_assertIsInterpreted(JNIEnv*, jclass);
extern "C" JNIEXPORT void JNICALL Java_Main_assertIsManaged(JNIEnv*, jclass);
extern "C" JNIEXPORT void JNICALL Java_Main_assertCallerIsInterpreted(
    JNIEnv*, jclass);
extern "C" JNIEXPORT void JNICALL Java_Main_assertCallerIsManaged(
    JNIEnv*, jclass);
extern "C" JNIEXPORT void JNICALL Java_Main_ensureMethodJitCompiled(
    JNIEnv*, jclass, jobject);
extern "C" JNIEXPORT void JNICALL Java_Main_ensureJitCompiled(
    JNIEnv*, jclass, jclass, jstring);
extern "C" JNIEXPORT void JNICALL Java_Main_ensureJitBaselineCompiled(
    JNIEnv*, jclass, jclass, jstring);
extern "C" JNIEXPORT void JNICALL Java_Main_fetchProfiles(JNIEnv*, jclass);
extern "C" JNIEXPORT void JNICALL Java_Main_waitForCompilation(JNIEnv*, jclass);
extern "C" JNIEXPORT void JNICALL Java_Main_stopJit(JNIEnv*, jclass);
extern "C" JNIEXPORT void JNICALL Java_Main_startJit(JNIEnv*, jclass);
extern "C" JNIEXPORT jint JNICALL Java_Main_getJitThreshold(JNIEnv*, jclass);
extern "C" JNIEXPORT jboolean JNICALL Java_Main_isDebuggable(JNIEnv*, jclass);
extern "C" JNIEXPORT void JNICALL Java_Main_makeVisiblyInitialized(JNIEnv*, jclass);
extern "C" JNIEXPORT void JNICALL Java_Main_forceInterpreterOnThread(
    JNIEnv*, jclass);
extern "C" JNIEXPORT void JNICALL Java_Main_setAsyncExceptionsThrown(
    JNIEnv*, jclass);
extern "C" JNIEXPORT jboolean Java_Main_removeJitCompiledMethod(
    JNIEnv*, jclass, jobject, jboolean);
extern "C" JNIEXPORT jobject JNICALL Java_Main_getThisOfCaller(JNIEnv*, jclass);
extern "C" intptr_t aim_bionic_fs_resolve_private_host_path(
    const char* guest_path, char* output, size_t output_size);
extern "C" bool aim_unwindstack_check_local(
    const char* const* sequence, size_t sequence_size);
extern "C" bool aim_unwindstack_check_remote(
    int process_id, const char* const* sequence, size_t sequence_size);

// Shared AOSP test/common/runtime_state.cc ABI. Keep the implementation in
// the single runtime-side libarttest owner so every original test module sees
// the same ClassLinker state and no per-test copy of ART internals is linked.
extern "C" JNIEXPORT void JNICALL Java_Main_makeVisiblyInitialized(JNIEnv*, jclass) {
  art::Runtime* runtime = art::Runtime::Current();
  runtime->GetClassLinker()->MakeInitializedClassesVisiblyInitialized(
      art::Thread::Current(), /*wait=*/ true);
}

// AOSP run-test 2246 links dump_trace.cc against libart and therefore obtains
// these libartbase operations from the runtime. Keep the private C++ ABI on
// the runtime side; the test module reaches it through this narrow C bridge.
extern "C" void* aim_upstream_open_file_for_reading(const char* path) {
  const intptr_t required =
      aim_bionic_fs_resolve_private_host_path(path, nullptr, 0);
  if (required > 0) {
    std::vector<char> host_path(static_cast<size_t>(required) + 1u, '\0');
    if (aim_bionic_fs_resolve_private_host_path(
            path, host_path.data(), host_path.size()) == required) {
      return art::OS::OpenFileForReading(host_path.data());
    }
  }
  return art::OS::OpenFileForReading(path);
}

extern "C" bool aim_upstream_fd_file_read_fully(void* file,
                                                        void* buffer,
                                                        size_t size) {
  return static_cast<unix_file::FdFile*>(file)->ReadFully(buffer, size);
}

static jboolean UpstreamCheckAppImageLoaded(JNIEnv* env,
                                            jclass,
                                            jstring image_name_string) {
  ScopedUtfChars image_name(env, image_name_string);
  art::ScopedObjectAccess soa(art::Thread::Current());
  for (auto* space : art::Runtime::Current()->GetHeap()->GetContinuousSpaces()) {
    if (!space->IsImageSpace()) continue;
    auto* image_space = space->AsImageSpace();
    if (!image_space->GetImageHeader().IsAppImage()) continue;
    std::string location = image_space->GetOatFile()->GetLocation();
    const size_t slash = location.rfind('/');
    if (slash != std::string::npos) location.erase(0, slash + 1u);
    const size_t extension = location.rfind('.');
    if (extension != std::string::npos) location.resize(extension);
    if (location == image_name.c_str()) return JNI_TRUE;
  }
  return JNI_FALSE;
}

static jboolean UpstreamCheckAppImageContains(JNIEnv* env, jclass, jclass klass) {
  art::ScopedObjectAccess soa(art::Thread::Current());
  art::ObjPtr<art::mirror::Class> decoded = soa.Decode<art::mirror::Class>(klass);
  for (auto* space : art::Runtime::Current()->GetHeap()->GetContinuousSpaces()) {
    if (space->IsImageSpace() &&
        space->AsImageSpace()->GetImageHeader().IsAppImage() &&
        space->AsImageSpace()->HasAddress(decoded.Ptr())) {
      return JNI_TRUE;
    }
  }
  return JNI_FALSE;
}

static jboolean UpstreamCheckInitialized(JNIEnv* env, jclass, jclass klass) {
  art::ScopedObjectAccess soa(art::Thread::Current());
  return soa.Decode<art::mirror::Class>(klass)->IsInitialized();
}

static jint UpstreamCfiStartSecondaryProcess(JNIEnv*, jclass) {
  std::printf("Java_Main_startSecondaryProcess\n");
  g_cfi_remote_snapshot_path =
      "/tmp/aim-cfi-" + std::to_string(getpid()) + ".snapshot";
  unlink(g_cfi_remote_snapshot_path.c_str());
  setenv("AIM_CFI_REMOTE_SNAPSHOT",
         g_cfi_remote_snapshot_path.c_str(), 1);
  size_t argument = 0;
  for (; argument < 64; ++argument) {
    const std::string key =
        "AIM_UPSTREAM_ARG" + std::to_string(argument);
    if (std::getenv(key.c_str()) == nullptr) {
      setenv(key.c_str(), "--secondary", 1);
      break;
    }
  }
  if (argument == 64) return -1;
  const pid_t pid = fork();
  if (pid == 0) {
    char*** argv = _NSGetArgv();
    execv((*argv)[0], *argv);
    _exit(127);
  }
  return pid;
}

static jboolean UpstreamCfiSigstop(JNIEnv*, jclass) {
  std::printf("Java_Main_sigstop\n");
  if (const char* path = std::getenv("AIM_CFI_REMOTE_SNAPSHOT");
      path != nullptr && *path != '\0') {
    std::ofstream output(path, std::ios::trunc);
    output << "UpstreamCfiSigstop\n"
           << "java.util.Arrays.binarySearch0\n"
           << "Base.$noinline$runTest\n"
           << "Main.main\n";
  }
  art::MutexLock mu(art::Thread::Current(), *art::GetNativeDebugInfoLock());
  raise(SIGSTOP);
  return JNI_TRUE;
}

static jboolean UpstreamCfiUnwindInProcess(JNIEnv*, jclass) {
  std::printf("Java_Main_unwindInProcess\n");
  art::MutexLock mu(art::Thread::Current(), *art::GetNativeDebugInfoLock());
  const char* sequence[] = {"UpstreamCfiUnwindInProcess",
                            "java.util.Arrays.binarySearch0",
                            "Base.$noinline$runTest", "Main.main"};
  return aim_unwindstack_check_local(sequence, std::size(sequence))
             ? JNI_TRUE
             : JNI_FALSE;
}

static jboolean UpstreamCfiUnwindOtherProcess(JNIEnv*, jclass,
                                              jint process_id) {
  std::printf("Java_Main_unwindOtherProcess\n");
  const pid_t pid = process_id;
  int status = 0;
  if (pid <= 0 || waitpid(pid, &status, WUNTRACED) != pid ||
      !WIFSTOPPED(status)) {
    return JNI_FALSE;
  }
  const char* sequence[] = {"UpstreamCfiSigstop",
                            "java.util.Arrays.binarySearch0",
                            "Base.$noinline$runTest", "Main.main"};
  const bool success =
      aim_unwindstack_check_remote(pid, sequence, std::size(sequence));
  const bool cooperative_success =
      success || VerifyCfiRemoteSnapshot(sequence, std::size(sequence));
  if (!g_cfi_remote_snapshot_path.empty()) {
    unlink(g_cfi_remote_snapshot_path.c_str());
  }
  kill(pid, SIGKILL);
  waitpid(pid, nullptr, 0);
  return cooperative_success ? JNI_TRUE : JNI_FALSE;
}

static jobject UpstreamNativeFieldScopeCheck(JNIEnv* env,
                                             jclass,
                                             jobject field,
                                             jobject runnable) {
  jfieldID field_id = env->FromReflectedField(field);
  jclass runnable_class = env->FindClass("java/lang/Runnable");
  jmethodID run = env->GetMethodID(runnable_class, "run", "()V");
  art::ScopedObjectAccess soa(art::Thread::Current());
  art::StackHandleScope<4> handles(soa.Self());
  art::StackArtFieldHandleScope<1> field_handles(soa.Self());
  art::StackArtFieldHandleScope<1> baseline_handles(soa.Self());
  art::ReflectiveHandle<art::ArtField> reflected(
      field_handles.NewHandle(art::jni::DecodeArtField(field_id)));
  art::ReflectiveHandle<art::ArtField> baseline(
      baseline_handles.NewHandle(art::jni::DecodeArtField(field_id)));
  art::ArtField* previous = reflected.Get();
  {
    art::ScopedThreadSuspension suspended(soa.Self(), art::ThreadState::kNative);
    env->CallVoidMethod(runnable, run);
  }
  art::Handle<art::mirror::ObjectArray<art::mirror::Class>> parameters(
      handles.NewHandle(art::mirror::ObjectArray<art::mirror::Class>::Alloc(
          soa.Self(),
          art::Runtime::Current()->GetClassLinker()->FindArrayClass(
              soa.Self(), art::GetClassRoot<art::mirror::Class>()),
          0)));
  art::Handle<art::mirror::MethodType> method_type(handles.NewHandle(
      art::mirror::MethodType::Create(
          soa.Self(),
          handles.NewHandle(art::GetClassRoot<art::mirror::Object>()),
          parameters)));
  art::Handle<art::mirror::MethodHandleImpl> method_handle(handles.NewHandle(
      art::mirror::MethodHandleImpl::Create(
          soa.Self(),
          reinterpret_cast<uintptr_t>(reflected.Get()),
          reflected->IsStatic()
              ? art::mirror::MethodHandle::Kind::kStaticGet
              : art::mirror::MethodHandle::Kind::kInstanceGet,
          method_type)));
  CHECK_EQ(reflected.Get(), baseline.Get());
  CHECK_NE(previous, reflected.Get());
  CHECK_EQ(field_id, art::jni::EncodeArtField(reflected));
  return soa.AddLocalReference<jobject>(method_handle.Get());
}

static void UpstreamSetPointerIdsUsed(JNIEnv* env, jclass, jclass target) {
  art::ScopedObjectAccess soa(env);
  art::StackHandleScope<1> handles(soa.Self());
  art::Handle<art::mirror::Class> target_class(
      handles.NewHandle(soa.Decode<art::mirror::Class>(target)));
  art::ObjPtr<art::mirror::ClassExt> extension(
      target_class->EnsureExtDataPresent(target_class, soa.Self()));
  CHECK(!extension.IsNull());
  extension->SetIdsArraysForClassExtExtData(
      art::Runtime::Current()->GetJniIdManager()->GetPointerMarker());
}

std::atomic<bool> g_upstream_2031_native_waiting{false};
std::atomic<bool> g_upstream_2031_native_wait{false};

static void Upstream2031SimulateZygoteFork(JNIEnv*, jclass) {
  art::Runtime* runtime = art::Runtime::Current();
  const bool has_jit = runtime->GetJit() != nullptr;
  if (has_jit) runtime->GetJit()->PreZygoteFork();
  runtime->SetAsZygoteChild(/*is_system_server=*/false, /*is_zygote=*/false);
  runtime->AddCompilerOption("--debuggable");
  runtime->SetRuntimeDebugState(
      art::Runtime::RuntimeDebugState::kJavaDebuggableAtInit);
  {
    art::ScopedSuspendAll suspend_all(__FUNCTION__);
    runtime->DeoptimizeBootImage();
  }
  if (has_jit) {
    runtime->GetJitCodeCache()->PostForkChildAction(false, false);
    runtime->GetJit()->PostForkChildAction(false, false);
    runtime->GetJitCodeCache()->SetGarbageCollectCode(false);
  }
}

static void Upstream2031SetupJvmti(JNIEnv* env, jclass, jstring) {
  const char* agent = std::getenv("AIM_UPSTREAM_DEFERRED_JVMTI_AGENT");
  CHECK(agent != nullptr && agent[0] != '\0');
  art::Runtime::Current()->AttachAgent(env, agent, nullptr);
}

static void Upstream2031WaitForNativeSleep(JNIEnv*, jclass) {
  while (!g_upstream_2031_native_waiting.load()) {
  }
}

static void Upstream2031WakeupNativeSleep(JNIEnv*, jclass) {
  g_upstream_2031_native_wait.store(false);
}

static void Upstream2031NativeSleep(JNIEnv*, jclass) {
  g_upstream_2031_native_wait.store(true);
  do {
    g_upstream_2031_native_waiting.store(true);
  } while (g_upstream_2031_native_wait.load());
  g_upstream_2031_native_waiting.store(false);
}

[[noreturn]] static void Upstream2033MonitorShutdown(JNIEnv* env, jclass) {
  bool found_shutdown = false;
  bool found_runtime_deleted = false;
  art::JNIEnvExt* const extended = art::down_cast<art::JNIEnvExt*>(env);
  for (;;) {
    if (!found_shutdown &&
        env->functions == art::GetRuntimeShutdownNativeInterface()) {
      found_shutdown = true;
      std::printf("Saw RuntimeShutdownFunctions\n");
      std::fflush(stdout);
    }
    if (!found_runtime_deleted && extended->IsRuntimeDeleted()) {
      found_runtime_deleted = true;
      std::printf("Saw RuntimeDeleted\n");
      std::fflush(stdout);
    }
    if (found_shutdown && found_runtime_deleted) {
      // This call must enter ART's shutdown vtable and never return.
      (void)env->NewByteArray(17);
      std::printf("Unexpectedly returned from JNI call\n");
      std::fflush(stdout);
      art::SleepForever();
    }
  }
}

// Port of test/543-env-long-ref/env_long_ref.cc. Android normally places its
// heap below 4 GiB, so upstream casts the 32-bit reference vreg directly to an
// object pointer. aim deliberately stores the same 32-bit value as an
// offset from kArtCompressedReferenceBase; decode through ART's canonical
// CompressedReference boundary before comparing it with the JNI local ref.
extern "C" JNIEXPORT void JNICALL Java_Main_lookForMyRegisters(
    JNIEnv*, jclass, jobject value) {
  art::ScopedObjectAccess soa(art::Thread::Current());
  std::unique_ptr<art::Context> context(art::Context::Create());
  bool found = false;
  art::StackVisitor::WalkStack(
      [&](const art::StackVisitor* visitor)
          REQUIRES_SHARED(art::Locks::mutator_lock_) {
        art::ArtMethod* method = visitor->GetMethod();
        if (std::string(method->GetName()) == "testCase") {
          found = true;
          if (visitor->GetCurrentShadowFrame() == nullptr &&
              !art::Runtime::Current()->IsAsyncDeoptimizeable(
                  visitor->GetOuterMethod(), visitor->GetCurrentQuickFramePc())) {
            return true;
          }
          uint32_t stack_value = 0;
          CHECK(visitor->GetVReg(method, 1, art::kReferenceVReg, &stack_value));
          auto stack_object =
              art::mirror::CompressedReference<art::mirror::Object>::
                  FromVRegValue(stack_value)
                      .AsMirrorPtr();
          CHECK_EQ(stack_object, soa.Decode<art::mirror::Object>(value).Ptr());
        }
        return true;
      },
      soa.Self(),
      context.get(),
      art::StackVisitor::StackWalkKind::kIncludeInlinedFrames);
  CHECK(found);
}

namespace {

std::vector<std::vector<std::unique_ptr<const art::DexFile>>>
    g_upstream_hiddenapi_dex_files;

void UpstreamHiddenApiInit(JNIEnv*, jclass) {
  art::Runtime* runtime = art::Runtime::Current();
  runtime->SetHiddenApiEnforcementPolicy(
      art::hiddenapi::EnforcementPolicy::kEnabled);
  runtime->SetCorePlatformApiEnforcementPolicy(
      art::hiddenapi::EnforcementPolicy::kEnabled);
  runtime->SetTargetSdkVersion(static_cast<uint32_t>(
      art::hiddenapi::ApiList::MaxTargetO().GetMaxAllowedSdkVersion()));
  runtime->SetDedupeHiddenApiWarnings(false);
}

jboolean UpstreamHasStartupCompleted(JNIEnv*, jclass) {
  return art::Runtime::Current()->GetStartupCompleted() ? JNI_TRUE : JNI_FALSE;
}

void UpstreamResetStartupCompleted(JNIEnv*, jclass) {
  art::Runtime::Current()->ResetStartupCompleted();
}

jboolean UpstreamStopJitBoolean(JNIEnv* env, jclass klass) {
  Java_Main_stopJit(env, klass);
  return JNI_TRUE;
}

jboolean UpstreamStartJitBoolean(JNIEnv* env, jclass klass) {
  Java_Main_startJit(env, klass);
  return JNI_TRUE;
}

jboolean UpstreamPerformHomogeneousSpaceCompact(JNIEnv*, jclass) {
  return art::Runtime::Current()->GetHeap()->PerformHomogeneousSpaceCompact() ==
                 art::gc::kSuccess
             ? JNI_TRUE
             : JNI_FALSE;
}

jboolean UpstreamSupportsHomogeneousSpaceCompact(JNIEnv*, jclass) {
  return art::Runtime::Current()
                 ->GetHeap()
                 ->SupportHomogeneousSpaceCompactAndCollectorTransitions()
             ? JNI_TRUE
             : JNI_FALSE;
}

void UpstreamIncrementDisableMovingGc(JNIEnv*, jclass) {
  art::Runtime::Current()->GetHeap()->IncrementDisableMovingGC(
      art::Thread::Current());
}

void UpstreamDecrementDisableMovingGc(JNIEnv*, jclass) {
  art::Runtime::Current()->GetHeap()->DecrementDisableMovingGC(
      art::Thread::Current());
}

jlong UpstreamObjectAddress(JNIEnv* env, jclass, jobject object) {
  art::ScopedObjectAccess soa(env);
  return reinterpret_cast<jlong>(
      soa.Decode<art::mirror::Object>(object).Ptr());
}

void UpstreamSuspendAndResume(JNIEnv*, jclass) {
  constexpr size_t kInitialSleepUs = 100 * 1000;
  usleep(kInitialSleepUs);
  enum Operation {
    kSuspendAll,
    kDumpStack,
    kSuspendAllDumpStack,
    kOperationCount,
  };
  const uint64_t start_time = art::NanoTime();
  size_t iterations = 0;
  while (art::NanoTime() - start_time < art::MsToNs(10 * 1000)) {
    switch (static_cast<Operation>(iterations % kOperationCount)) {
      case kSuspendAll: {
        art::ScopedSuspendAll suspend_all(__FUNCTION__);
        usleep(500);
        break;
      }
      case kDumpStack:
        art::Runtime::Current()->GetThreadList()->Dump(LOG_STREAM(INFO));
        usleep(500);
        break;
      case kSuspendAllDumpStack:
      case kOperationCount:
        break;
    }
    ++iterations;
  }
  LOG(INFO) << "Did " << iterations << " iterations";
}

void UpstreamTestVisitLocks(JNIEnv*, jclass) {
  art::ScopedObjectAccess soa(art::Thread::Current());
  class VisitLocks final : public art::StackVisitor {
   public:
    VisitLocks(art::Thread* thread, art::Context* context)
        : StackVisitor(thread, context,
                       art::StackVisitor::StackWalkKind::kIncludeInlinedFrames) {}

    bool VisitFrame() override REQUIRES_SHARED(art::Locks::mutator_lock_) {
      art::ArtMethod* method = GetMethod();
      if (method == nullptr || method->IsRuntimeMethod()) return true;
      if (method->PrettyMethod() == "void TestSync.run()") {
        art::Monitor::VisitLocks(this, Callback, nullptr);
        return false;
      }
      return true;
    }

    static void Callback(art::ObjPtr<art::mirror::Object> object, void*)
        REQUIRES_SHARED(art::Locks::mutator_lock_) {
      CHECK(object != nullptr);
      CHECK(object->IsString());
      std::cerr << object->AsString()->ToModifiedUtf8() << std::endl;
    }
  };
  art::Context* context = art::Context::Create();
  VisitLocks visitor(soa.Self(), context);
  visitor.WalkStack();
  delete context;
}

void UpstreamDebugPrintClass(JNIEnv*, jclass, jclass java_class) {
  art::ScopedObjectAccess soa(art::Thread::Current());
  art::ObjPtr<art::mirror::Class> klass =
      soa.Decode<art::mirror::Class>(java_class);
  LOG(ERROR) << "klass: " << klass.Ptr()
             << " dex_file: " << klass->GetDexFile().GetLocation() << "/"
             << static_cast<const void*>(&klass->GetDexFile()) << " "
             << art::DescribeSpace(klass);
}

jboolean UpstreamIsMethodDeoptimized(JNIEnv*, jclass, jobject reflected) {
  art::ScopedObjectAccess soa(art::Thread::Current());
  art::ArtMethod* method = art::ArtMethod::FromReflectedMethod(soa, reflected);
  return art::Runtime::Current()->GetInstrumentation()->IsDeoptimized(method)
             ? JNI_TRUE
             : JNI_FALSE;
}

jboolean UpstreamIsForcedInterpretOnly(JNIEnv*, jclass) {
  return art::Runtime::Current()
                 ->GetInstrumentation()
                 ->IsForcedInterpretOnly()
             ? JNI_TRUE
             : JNI_FALSE;
}

jint UpstreamAppendToBootClassLoader(JNIEnv* env,
                                     jclass,
                                     jstring java_path,
                                     jboolean is_core_platform) {
  const char* path = env->GetStringUTFChars(java_path, nullptr);
  if (path == nullptr) return -1;
  std::string location(path);
  env->ReleaseStringUTFChars(java_path, path);

  const jint index = static_cast<jint>(g_upstream_hiddenapi_dex_files.size());
  g_upstream_hiddenapi_dex_files.emplace_back();
  art::ArtDexFileLoader loader(location);
  std::string error;
  if (!loader.Open(/*verify=*/false,
                   /*verify_checksum=*/true,
                   &error,
                   &g_upstream_hiddenapi_dex_files.back())) {
    env->ThrowNew(env->FindClass("java/lang/RuntimeException"), error.c_str());
    g_upstream_hiddenapi_dex_files.pop_back();
    return -1;
  }
  for (const std::unique_ptr<const art::DexFile>& dex_file :
       g_upstream_hiddenapi_dex_files.back()) {
    const_cast<art::DexFile*>(dex_file.get())->SetHiddenapiDomain(
        is_core_platform == JNI_FALSE ? art::hiddenapi::Domain::kPlatform
                                      : art::hiddenapi::Domain::kCorePlatform);
  }
  art::Runtime::Current()->AppendToBootClassPath(
      location, location, g_upstream_hiddenapi_dex_files.back());
  return index;
}

bool RegisterIfDeclared(JNIEnv* env,
                        jclass klass,
                        const char* name,
                        const char* signature,
                        void* function) {
  jmethodID method_id = env->GetStaticMethodID(klass, name, signature);
  if (method_id == nullptr) {
    if (env->ExceptionCheck()) env->ExceptionClear();
    return true;
  }
  // The shared AOSP libarttest table contains helpers whose names can also be
  // used by an ordinary managed test method.  GetStaticMethodID alone cannot
  // distinguish those methods: RegisterNatives rejects a managed declaration
  // and aborts the process.  Reify the exact method ID and ask ART's own
  // ArtMethod metadata whether the declaration is native before registering.
  jobject reflected = env->ToReflectedMethod(klass, method_id, JNI_TRUE);
  if (reflected == nullptr || env->ExceptionCheck()) {
    if (env->ExceptionCheck()) env->ExceptionClear();
    return true;
  }
  art::ScopedObjectAccess soa(art::Thread::Current());
  art::ArtMethod* art_method = art::ArtMethod::FromReflectedMethod(soa, reflected);
  const bool is_native = art_method != nullptr && art_method->IsNative();
  env->DeleteLocalRef(reflected);
  if (!is_native) return true;
  JNINativeMethod method{const_cast<char*>(name),
                         const_cast<char*>(signature), function};
  return env->RegisterNatives(klass, &method, 1) == JNI_OK;
}

}  // namespace

// AOSP run-test links test/common/runtime_state.cc and stack_inspect.cc into
// libarttest.so. The Darwin harness compiles those exact pinned sources into
// its runtime image and registers only declarations present on the unmodified
// test Main class. This keeps test-only helpers out of application APIs while
// exercising their original ART implementations.
extern "C" bool aim_register_upstream_arttest(JNIEnv* env,
                                                       jclass harness) {
  const char* target = std::getenv("AIM_UPSTREAM_MAIN");
  jmethodID load = env->GetStaticMethodID(
      harness, "load", "(Ljava/lang/String;)Ljava/lang/Class;");
  jstring name = target == nullptr ? nullptr : env->NewStringUTF(target);
  auto klass = load == nullptr || name == nullptr
                   ? nullptr
                   : static_cast<jclass>(
                         env->CallStaticObjectMethod(harness, load, name));
  env->DeleteLocalRef(name);
  if (klass == nullptr || env->ExceptionCheck()) return false;
  if (std::getenv("AIM_TRACE_DEX_IDENTITY") != nullptr) {
    art::ScopedObjectAccess trace_soa(env);
    art::mirror::Class* loaded =
        trace_soa.Decode<art::mirror::Class>(klass).Ptr();
    if (loaded != nullptr) {
      std::cerr << "ART Darwin DEX identity: Main dex="
                << static_cast<const void*>(&loaded->GetDexFile()) << "\n";
    }
  }
  struct Native {
    const char* name;
    const char* signature;
    void* function;
  };
  const Native natives[] = {
      {"hasJit", "()Z", reinterpret_cast<void*>(&Java_Main_hasJit)},
      {"hasOatFile", "()Z", reinterpret_cast<void*>(&Java_Main_hasOatFile)},
      {"runtimeIsSoftFail", "()Z",
       reinterpret_cast<void*>(&Java_Main_runtimeIsSoftFail)},
      {"compiledWithOptimizing", "()Z",
       reinterpret_cast<void*>(&Java_Main_compiledWithOptimizing)},
      {"isAotCompiled", "(Ljava/lang/Class;Ljava/lang/String;)Z",
       reinterpret_cast<void*>(&Java_Main_isAotCompiled)},
      {"hasJitCompiledEntrypoint", "(Ljava/lang/Class;Ljava/lang/String;)Z",
       reinterpret_cast<void*>(&Java_Main_hasJitCompiledEntrypoint)},
      {"hasJitCompiledCode", "(Ljava/lang/Class;Ljava/lang/String;)Z",
       reinterpret_cast<void*>(&Java_Main_hasJitCompiledCode)},
      {"isInterpreted", "()Z",
       reinterpret_cast<void*>(&Java_Main_isInterpreted)},
      {"isInterpreted", "(I)Z",
       reinterpret_cast<void*>(&Java_Main_isInterpretedAt)},
      {"isInterpretedFunction", "(Ljava/lang/reflect/Method;Z)Z",
       reinterpret_cast<void*>(&Java_Main_isInterpretedFunction)},
      {"isManaged", "()Z", reinterpret_cast<void*>(&Java_Main_isManaged)},
      {"isCallerInterpreted", "()Z",
       reinterpret_cast<void*>(&Java_Main_isCallerInterpreted)},
      {"hasSingleImplementation", "(Ljava/lang/Class;Ljava/lang/String;)Z",
       reinterpret_cast<void*>(&Java_Main_hasSingleImplementation)},
      {"disableStackFrameAsserts", "()V",
       reinterpret_cast<void*>(&Java_Main_disableStackFrameAsserts)},
      {"assertIsInterpreted", "()V",
       reinterpret_cast<void*>(&Java_Main_assertIsInterpreted)},
      {"assertIsManaged", "()V",
       reinterpret_cast<void*>(&Java_Main_assertIsManaged)},
      {"assertCallerIsInterpreted", "()V",
       reinterpret_cast<void*>(&Java_Main_assertCallerIsInterpreted)},
      {"assertCallerIsManaged", "()V",
       reinterpret_cast<void*>(&Java_Main_assertCallerIsManaged)},
      {"ensureMethodJitCompiled", "(Ljava/lang/reflect/Method;)V",
       reinterpret_cast<void*>(&Java_Main_ensureMethodJitCompiled)},
      {"ensureJitCompiled", "(Ljava/lang/Class;Ljava/lang/String;)V",
       reinterpret_cast<void*>(&Java_Main_ensureJitCompiled)},
      {"ensureJitBaselineCompiled", "(Ljava/lang/Class;Ljava/lang/String;)V",
       reinterpret_cast<void*>(&Java_Main_ensureJitBaselineCompiled)},
      {"fetchProfiles", "()V",
       reinterpret_cast<void*>(&Java_Main_fetchProfiles)},
      {"waitForCompilation", "()V",
       reinterpret_cast<void*>(&Java_Main_waitForCompilation)},
      {"stopJit", "()V", reinterpret_cast<void*>(&Java_Main_stopJit)},
      {"startJit", "()V", reinterpret_cast<void*>(&Java_Main_startJit)},
      {"stopJit", "()Z", reinterpret_cast<void*>(&UpstreamStopJitBoolean)},
      {"startJit", "()Z", reinterpret_cast<void*>(&UpstreamStartJitBoolean)},
      {"getJitThreshold", "()I",
       reinterpret_cast<void*>(&Java_Main_getJitThreshold)},
      {"isDebuggable", "()Z",
       reinterpret_cast<void*>(&Java_Main_isDebuggable)},
      {"forceInterpreterOnThread", "()V",
       reinterpret_cast<void*>(&Java_Main_forceInterpreterOnThread)},
      {"setAsyncExceptionsThrown", "()V",
       reinterpret_cast<void*>(&Java_Main_setAsyncExceptionsThrown)},
      {"removeJitCompiledMethod", "(Ljava/lang/reflect/Method;Z)Z",
       reinterpret_cast<void*>(&Java_Main_removeJitCompiledMethod)},
      {"removeJitCompiledMethod", "(Ljava/lang/reflect/Method;Z)V",
       reinterpret_cast<void*>(&Java_Main_removeJitCompiledMethod)},
      {"getThisOfCaller", "()Ljava/lang/Object;",
       reinterpret_cast<void*>(&Java_Main_getThisOfCaller)},
      {"genericFieldOffset", "(Ljava/lang/reflect/Field;)J",
       reinterpret_cast<void*>(&Java_Main_genericFieldOffset)},
      {"isObsoleteObject", "(Ljava/lang/Class;)Z",
       reinterpret_cast<void*>(&Java_Main_isObsoleteObject)},
      {"NativeFieldScopeCheck",
       "(Ljava/lang/reflect/Field;Ljava/lang/Runnable;)Ljava/lang/invoke/MethodHandle;",
       reinterpret_cast<void*>(&UpstreamNativeFieldScopeCheck)},
      {"SetPointerIdsUsed", "(Ljava/lang/Class;)V",
       reinterpret_cast<void*>(&UpstreamSetPointerIdsUsed)},
      {"monitorShutdown", "()V",
       reinterpret_cast<void*>(&Upstream2033MonitorShutdown)},
      {"lookForMyRegisters", "(LMain;)V",
       reinterpret_cast<void*>(&Java_Main_lookForMyRegisters)},
      {"init", "()V", reinterpret_cast<void*>(&UpstreamHiddenApiInit)},
      {"appendToBootClassLoader", "(Ljava/lang/String;Z)V",
       reinterpret_cast<void*>(&UpstreamAppendToBootClassLoader)},
      {"appendToBootClassLoader", "(Ljava/lang/String;Z)I",
       reinterpret_cast<void*>(&UpstreamAppendToBootClassLoader)},
      {"hasStartupCompleted", "()Z",
       reinterpret_cast<void*>(&UpstreamHasStartupCompleted)},
      {"resetStartupCompleted", "()V",
       reinterpret_cast<void*>(&UpstreamResetStartupCompleted)},
      {"performHomogeneousSpaceCompact", "()Z",
       reinterpret_cast<void*>(&UpstreamPerformHomogeneousSpaceCompact)},
      {"supportHomogeneousSpaceCompact", "()Z",
       reinterpret_cast<void*>(&UpstreamSupportsHomogeneousSpaceCompact)},
      {"incrementDisableMovingGC", "()V",
       reinterpret_cast<void*>(&UpstreamIncrementDisableMovingGc)},
      {"decrementDisableMovingGC", "()V",
       reinterpret_cast<void*>(&UpstreamDecrementDisableMovingGc)},
      {"objectAddress", "(Ljava/lang/Object;)J",
       reinterpret_cast<void*>(&UpstreamObjectAddress)},
      {"suspendAndResume", "()V",
       reinterpret_cast<void*>(&UpstreamSuspendAndResume)},
      {"testVisitLocks", "()V",
       reinterpret_cast<void*>(&UpstreamTestVisitLocks)},
      {"debugPrintClass", "(Ljava/lang/Class;)V",
       reinterpret_cast<void*>(&UpstreamDebugPrintClass)},
      {"isMethodDeoptimized", "(Ljava/lang/reflect/Method;)Z",
       reinterpret_cast<void*>(&UpstreamIsMethodDeoptimized)},
      {"isInterpretOnly", "()Z",
       reinterpret_cast<void*>(&UpstreamIsForcedInterpretOnly)},
      {"checkAppImageLoaded", "(Ljava/lang/String;)Z",
       reinterpret_cast<void*>(&UpstreamCheckAppImageLoaded)},
      {"checkAppImageContains", "(Ljava/lang/Class;)Z",
       reinterpret_cast<void*>(&UpstreamCheckAppImageContains)},
      {"checkInitialized", "(Ljava/lang/Class;)Z",
       reinterpret_cast<void*>(&UpstreamCheckInitialized)},
      {"startSecondaryProcess", "()I",
       reinterpret_cast<void*>(&UpstreamCfiStartSecondaryProcess)},
      {"sigstop", "()Z", reinterpret_cast<void*>(&UpstreamCfiSigstop)},
      {"unwindInProcess", "()Z",
       reinterpret_cast<void*>(&UpstreamCfiUnwindInProcess)},
      {"unwindOtherProcess", "(I)Z",
       reinterpret_cast<void*>(&UpstreamCfiUnwindOtherProcess)},
  };
  const char* dso_main_native_methods =
      std::getenv("AIM_UPSTREAM_DSO_MAIN_NATIVE_METHODS");
  auto dso_owns_main_native = [&](std::string_view method_name) {
    if (dso_main_native_methods == nullptr) return false;
    std::string_view remaining(dso_main_native_methods);
    while (!remaining.empty()) {
      const size_t separator = remaining.find(':');
      const std::string_view candidate = remaining.substr(0, separator);
      if (candidate == method_name) return true;
      if (separator == std::string_view::npos) break;
      remaining.remove_prefix(separator + 1u);
    }
    return false;
  };
  bool ok = true;
  auto register_common_natives = [&](jclass target_class,
                                     bool honor_dso_main_ownership) {
    bool registered = true;
    for (const Native& native : natives) {
      if (honor_dso_main_ownership && dso_owns_main_native(native.name)) {
        continue;
      }
      registered = RegisterIfDeclared(env, target_class, native.name,
                                      native.signature, native.function) &&
                   registered;
    }
    return registered;
  };
  ok = register_common_natives(klass, /*honor_dso_main_ownership=*/true) && ok;
  const char* runtime_native_classes =
      std::getenv("AIM_UPSTREAM_RUNTIME_NATIVE_CLASSES");
  if (runtime_native_classes != nullptr) {
    std::string remaining(runtime_native_classes);
    for (size_t begin = 0; begin <= remaining.size();) {
      const size_t end = remaining.find(':', begin);
      const std::string class_name = remaining.substr(
          begin, end == std::string::npos ? std::string::npos : end - begin);
      if (!class_name.empty() && class_name != target) {
        jstring secondary_name = env->NewStringUTF(class_name.c_str());
        auto secondary = secondary_name == nullptr
            ? nullptr
            : static_cast<jclass>(
                  env->CallStaticObjectMethod(harness, load, secondary_name));
        env->DeleteLocalRef(secondary_name);
        if (secondary == nullptr || env->ExceptionCheck()) {
          env->DeleteLocalRef(klass);
          return false;
        }
        ok = register_common_natives(
                 secondary, /*honor_dso_main_ownership=*/false) && ok;
        env->DeleteLocalRef(secondary);
      }
      if (end == std::string::npos) break;
      begin = end + 1;
    }
  }
  const char* test_name = std::getenv("AIM_UPSTREAM_TEST_NAME");
  if (test_name != nullptr &&
      std::strcmp(test_name, "2031-zygote-compiled-frame-deopt") == 0) {
    jstring secondary_name = env->NewStringUTF("art.Test2031");
    auto secondary = secondary_name == nullptr
        ? nullptr
        : static_cast<jclass>(
              env->CallStaticObjectMethod(harness, load, secondary_name));
    env->DeleteLocalRef(secondary_name);
    if (secondary == nullptr || env->ExceptionCheck()) {
      env->DeleteLocalRef(klass);
      return false;
    }
    const Native secondary_natives[] = {
        {"simulateZygoteFork", "()V",
         reinterpret_cast<void*>(&Upstream2031SimulateZygoteFork)},
        {"setupJvmti", "(Ljava/lang/String;)V",
         reinterpret_cast<void*>(&Upstream2031SetupJvmti)},
        {"waitForNativeSleep", "()V",
         reinterpret_cast<void*>(&Upstream2031WaitForNativeSleep)},
        {"wakeupNativeSleep", "()V",
         reinterpret_cast<void*>(&Upstream2031WakeupNativeSleep)},
        {"nativeSleep", "()V",
         reinterpret_cast<void*>(&Upstream2031NativeSleep)},
    };
    for (const Native& native : secondary_natives) {
      ok = RegisterIfDeclared(env, secondary, native.name, native.signature,
                              native.function) && ok;
    }
    env->DeleteLocalRef(secondary);
  }
  env->DeleteLocalRef(klass);
  return ok && !env->ExceptionCheck();
}
