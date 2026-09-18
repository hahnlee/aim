#include "runtime_jni_acceptance_probe.h"

#include <iostream>
#include <iterator>
#include <cstdlib>
#include <cstring>
#include <atomic>
#include <chrono>
#include <thread>

#include "art_method-inl.h"
#include "art_field.h"
#include "class_linker.h"
#include "darwin_art/darwin_art.h"
#include "runtime.h"
#include "jit/jit.h"
#include "jit/jit_code_cache.h"
#include "darwin_jit_eligibility.h"
#include "gc/heap.h"
#include "runtime_abi_probe.h"
#include "jni/scoped_local_frame.h"
#include "handle_scope-inl.h"
#include "jni/jni_internal.h"
#include "mirror/class-inl.h"
#include "thread-current-inl.h"
#include "monitor.h"
#include "interpreter/interpreter.h"
#include "interpreter/mterp/nterp.h"
#include "entrypoints/runtime_asm_entrypoints.h"
#include <unwindstack/AndroidUnwinder.h>
#include "runtime_jit_exit_hook_acceptance.h"
#include "runtime_jit_fields_acceptance.h"
#include "runtime_jit_arrays_acceptance.h"
#include "runtime_jit_allocation_acceptance.h"
#include "runtime_jit_monitor_contention.h"
#include "runtime_jit_numeric_acceptance.h"
#include "runtime_jit_bits_acceptance.h"
#include "runtime_jit_specialized_intrinsics.h"
#include "runtime_jit_string_intrinsics.h"
#include "runtime_jit_string_hidden_intrinsics.h"
#include "runtime_jit_system_arraycopy.h"
#include "runtime_jit_math_hinvoke.h"
#include "runtime_jit_crc32.h"
#include "runtime_jit_memory.h"
#include "runtime_jit_reference_boxing.h"
#include "runtime_jit_unsafe_intrinsics.h"
#include "runtime_jit_compare_acceptance.h"
#include "runtime_jit_switch_acceptance.h"
#include "runtime_jit_type_acceptance.h"
#include "runtime_jit_cold_type_acceptance.h"
#include "runtime_jit_failed_initialization.h"
#include "runtime_jit_call_arguments.h"
#include "runtime_jit_composed_calls.h"
#include "runtime_jit_virtual_composed.h"
#include "runtime_jit_instance_body.h"
#include "runtime_jit_field_composed.h"
#include "runtime_jit_array_composed.h"
#include "runtime_jit_array_allocation_composed.h"
#include "runtime_jit_object_composed.h"
#include "runtime_jit_handlers.h"
#include "runtime_jit_nested_finally.h"
#include "runtime_jit_synchronized.h"
#include "runtime_jit_interface.h"
#include "runtime_jit_native_calls.h"
#include "runtime_jit_mixed_arguments.h"
#include "runtime_jit_failed_static.h"
#include "runtime_jit_cold_static.h"
#include "runtime_jit_clinit_moving_gc.h"
#include "runtime_jit_cold_static_write.h"
#include "runtime_jit_unresolved_static.h"
#include "runtime_jit_unresolved_instance.h"
#include "runtime_jit_unresolved_calls.h"
#include "runtime_jit_super.h"
#include "runtime_jit_large_method.h"
#include "runtime_jit_array_literals.h"
#include "runtime_jit_polymorphic.h"
#include "runtime_jit_invoke_custom.h"
#include "runtime_jit_recursive_initialization.h"
#include "runtime_jit_concurrent_initialization.h"
#include "runtime_jit_int_div_acceptance.h"
#include "runtime_jit_scalar_loop_acceptance.h"
#include "runtime_jit_osr_acceptance.h"
#include "runtime_jit_osr_gc_acceptance.h"
#include "runtime_jit_osr_deopt_acceptance.h"

namespace darwin_art_jni_acceptance_phase {

static std::atomic<uint32_t> native_unwind_calls{0};

static jobject NativeReceiverIdentity(JNIEnv* env, jobject receiver) {
  return env->NewLocalRef(receiver);
}

static jobject NativeReferenceIdentity(JNIEnv* env, jclass owner, jobject value) {
  if (native_callback_mode != 0) {
    jmethodID callback = env->GetStaticMethodID(owner, "jitGcTarget", "(Ljava/lang/Object;)Ljava/lang/Object;");
    if (!callback || env->ExceptionCheck()) return nullptr;
    uint64_t before = art::Runtime::Current()->GetHeap()->GetGcCount();
    jobject result = env->CallStaticObjectMethod(owner, callback, value);
    native_callback_gc_observed = art::Runtime::Current()->GetHeap()->GetGcCount() > before;
    if (env->ExceptionCheck()) return nullptr;
    if (native_callback_mode == 2) {
      jmethodID throwing = env->GetStaticMethodID(owner, "jitThrow", "(Ljava/lang/Throwable;)V");
      if (!throwing || env->ExceptionCheck()) return nullptr;
      env->CallStaticVoidMethod(owner, throwing, result);
      return nullptr;  // Preserve the callback's pending exception through JNI exit.
    }
    return result;
  }
  return env->NewLocalRef(value);
}

static jint NativeUnwindJit(JNIEnv*, jclass) {
  native_unwind_calls.fetch_add(1, std::memory_order_relaxed);
  unwindstack::AndroidLocalUnwinder unwinder;
  unwindstack::AndroidUnwinderData data;
  if (!unwinder.Unwind(data)) {
    std::cerr << "ART JIT unwind: AndroidLocalUnwinder failed error="
              << static_cast<unsigned>(data.error.code) << " detail=" << data.error.address
              << "\n";
    return 0;
  }
  for (const auto& frame : data.frames) {
    if (std::strstr(frame.function_name.c_str(), "Hello.jitUnwindBridge") != nullptr) return 1;
  }
  std::cerr << "ART JIT unwind: compiled bridge not symbolized; frames=" << data.frames.size()
            << "\n";
  for (const auto& frame : data.frames) {
    std::cerr << "  pc=0x" << std::hex << frame.pc << std::dec
              << " function=" << frame.function_name.c_str() << "\n";
  }
  return 0;
}

int run(JNIEnv* env, art::Thread* self, art::ClassLinker* class_linker,
        art::Handle<art::mirror::Class> hello, jclass hello_class,
        Results* results) {
  if (env == nullptr || self == nullptr || class_linker == nullptr ||
      hello.IsNull() || hello_class == nullptr || results == nullptr) {
    std::cerr << "ART Darwin JNI: acceptance inputs are missing\n";
    return 6;
  }

  JNINativeMethod native_methods[]{
      {const_cast<char*>("nativeVirtualReceiver"), const_cast<char*>("()Ljava/lang/Object;"),
       reinterpret_cast<void*>(&NativeReceiverIdentity)},
      {const_cast<char*>("nativeDirectReceiver"), const_cast<char*>("()Ljava/lang/Object;"),
       reinterpret_cast<void*>(&NativeReceiverIdentity)},
      {const_cast<char*>("nativeReferenceIdentity"),
       const_cast<char*>("(Ljava/lang/Object;)Ljava/lang/Object;"),
       reinterpret_cast<void*>(&NativeReferenceIdentity)},
      {const_cast<char*>("hostPageSize"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&darwin_art_jni_scope::HostPageSize)},
      {const_cast<char*>("nativeUnwindJit"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&NativeUnwindJit)},
      {const_cast<char*>("nativePackedIntegerStack"),
       const_cast<char*>("(IIIIIIIJII)J"),
       reinterpret_cast<void*>(&darwin_art_abi_probe::packed_integer_stack)},
      {const_cast<char*>("nativePackedFloatingStack"),
       const_cast<char*>("(FFFFFFFFFD)J"),
       reinterpret_cast<void*>(&darwin_art_abi_probe::packed_floating_stack)},
      {const_cast<char*>("nativePackedReferenceStack"),
       const_cast<char*>("(IIIIIIILjava/lang/Object;I)J"),
       reinterpret_cast<void*>(&darwin_art_abi_probe::packed_reference_stack)},
      {const_cast<char*>("nativePackedNarrowStack"),
       const_cast<char*>("(IIIIIIZBCSIJ)J"),
       reinterpret_cast<void*>(&darwin_art_abi_probe::packed_narrow_stack)},
  };
  if (env->RegisterNatives(hello_class, native_methods,
                           std::size(native_methods)) != JNI_OK) {
    std::cerr << "ART Darwin JNI: RegisterNatives failed\n";
    return 6;
  }
  if (!class_linker->EnsureInitialized(self, hello, true, true)) {
    std::cerr << "ART Darwin JNI: Hello initialization failed\n";
    return 7;
  }
  {
    // ARM64 publishes initialized classes in batches. Flush that ordinary
    // AOSP boundary before inspecting the cold managed entrypoint so the
    // class-linker has replaced its temporary resolution trampoline.
    art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
    class_linker->MakeInitializedClassesVisiblyInitialized(self, true);
  }
  art::ArtMethod* answer =
      hello->FindClassMethod("answer", "()I", art::kRuntimePointerSize);
  if (answer == nullptr) {
    std::cerr << "ART Darwin DEX: answer()I lookup failed\n";
    return 8;
  }

  const void* nterp_entry = art::interpreter::GetNterpEntryPoint();
  const void* initial_entry = answer->GetEntryPointFromQuickCompiledCode();
  const bool can_use_nterp = art::interpreter::CanRuntimeUseNterp();
  // AOSP deliberately disables nterp for debuggable/instrumented runtimes
  // and routes methods through the quick-to-interpreter bridge instead. The
  // probe must validate that platform policy rather than treating it as an
  // admission failure for a real debuggable APK.
  const bool valid_entry = can_use_nterp
      ? initial_entry == nterp_entry
      : initial_entry == art::GetQuickToInterpreterBridge();
  if (!art::Runtime::Current()->IsStarted() ||
      !art::interpreter::IsNterpSupported() ||
      nterp_entry == nullptr || !valid_entry) {
    std::cerr << "ART Nterp acceptance: admission failed started="
              << art::Runtime::Current()->IsStarted()
              << " supported="
              << art::interpreter::IsNterpSupported()
              << " runtime=" << art::interpreter::CanRuntimeUseNterp()
              << " expected=" << nterp_entry
              << " actual=" << initial_entry << "\n";
    return 8;
  }

  art::JValue result;
  answer->Invoke(self, /* args= */ nullptr, /* args_size= */ 0u, &result, "I");
  if (self->IsExceptionPending()) {
    std::cerr << "ART Darwin DEX: answer()I threw\n";
    return 9;
  }
  if (result.GetI() != 42) {
    std::cerr << "ART Darwin DEX: expected 42, got " << result.GetI() << "\n";
    return 10;
  }
  if (can_use_nterp &&
      answer->GetEntryPointFromQuickCompiledCode() != nterp_entry) {
    std::cerr << "ART Nterp acceptance: cold invocation changed entrypoint expected="
              << nterp_entry << " actual="
              << answer->GetEntryPointFromQuickCompiledCode() << "\n";
    return 10;
  }
  std::cerr << "ART Nterp acceptance: AOSP admission and native interpreter execution PASS"
            << " entry=" << nterp_entry << " result=" << result.GetI() << "\n";

  const char* jit_request = std::getenv("DARWIN_ART_JIT");
  if (jit_request != nullptr && std::strcmp(jit_request, "1") == 0 &&
      art::Runtime::Current()->GetJit() == nullptr) {
    std::cerr << "ART JIT acceptance: explicitly requested compiler unavailable\n";
    return 79;
  }
  if (auto* jit = art::Runtime::Current()->GetJit();
      std::getenv("DARWIN_ART_JIT_ACCEPTANCE_ONLY") != nullptr &&
      jit != nullptr && jit->UseJitCompilation()) {
    auto* arithmetic = hello->FindClassMethod("jitArithmetic", "(II)I", art::kRuntimePointerSize);
    if (arithmetic != nullptr) {
      std::cerr << "ART JIT eligibility: verified=" << hello->IsVerified()
                << " visible=" << hello->IsVisiblyInitialized()
                << " skip_checks=" << arithmetic->SkipAccessChecks()
                << " eligible=" << art::jit::DarwinJitCanCompile(arithmetic, art::CompilationKind::kOptimized)
                << "\n";
    }
    if (arithmetic == nullptr ||
        !jit->CompileMethod(arithmetic, self, art::CompilationKind::kOptimized, false)) {
      std::cerr << "ART JIT acceptance: arithmetic compilation failed\n";
      return 80;
    }
    const void* entry = arithmetic->GetEntryPointFromQuickCompiledCode();
    if (!jit->GetCodeCache()->ContainsPc(entry) || !jit->GetCodeCache()->ContainsMethod(arithmetic)) {
      std::cerr << "ART JIT acceptance: entry is not in registered code cache\n";
      return 81;
    }
    auto* unwind_bridge =
        hello->FindClassMethod("jitUnwindBridge", "()I", art::kRuntimePointerSize);
    if (unwind_bridge == nullptr ||
        !jit->CompileMethod(unwind_bridge, self, art::CompilationKind::kOptimized, false)) {
      std::cerr << "ART JIT unwind: bridge compilation failed\n";
      return 151;
    }
    art::JValue unwind_result;
    unwind_bridge->Invoke(self, nullptr, 0u, &unwind_result, "I");
    if (self->IsExceptionPending() || unwind_result.GetI() != 1) {
      std::cerr << "ART JIT unwind: mixed Mach-O/JIT stack failed calls="
                << native_unwind_calls.load(std::memory_order_relaxed)
                << " result=" << unwind_result.GetI()
                << " exception=" << self->IsExceptionPending() << "\n";
      return 152;
    }
    std::cerr << "ART JIT unwind: compiled Java -> JNI -> Mach-O symbolization PASS\n";
    const uint32_t cases[][2] = {{0, 0}, {0xffffffffu, 1}, {0x80000000u, 33},
                                {0x7fffffffu, 0xffffffffu}, {12345, 17}};
    for (const auto& values : cases) {
      uint32_t args[] = {values[0], values[1]};
      art::JValue actual;
      arithmetic->Invoke(self, args, sizeof(args), &actual, "III");
      const uint32_t expected = (args[0] * 31u + args[1]) ^ (args[0] >> (args[1] & 31));
      if (self->IsExceptionPending() || static_cast<uint32_t>(actual.GetI()) != expected) {
        std::cerr << "ART JIT acceptance: compiled arithmetic result mismatch\n";
        return 82;
      }
    }
    std::cerr << "ART JIT acceptance: AOSP compiled arithmetic PASS cases=5 entry=" << entry << "\n";
    {
      art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
      art::Runtime::Current()->GetHeap()->CollectGarbage(false);
    }
    for (uint32_t iteration = 0; iteration < 10000; ++iteration) {
      uint32_t args[] = {iteration * 0x9e3779b9u, iteration};
      art::JValue actual;
      arithmetic->Invoke(self, args, sizeof(args), &actual, "III");
      const uint32_t expected = (args[0] * 31u + args[1]) ^ (args[0] >> (args[1] & 31));
      if (self->IsExceptionPending() || static_cast<uint32_t>(actual.GetI()) != expected) return 83;
    }
    if (!jit->GetCodeCache()->ContainsPc(arithmetic->GetEntryPointFromQuickCompiledCode())) return 84;
    std::cerr << "ART JIT acceptance: post-GC compiled calls PASS iterations=10000\n";
    auto* identity = hello->FindClassMethod("jitIdentity", "(Ljava/lang/Object;)Ljava/lang/Object;",
                                          art::kRuntimePointerSize);
    jmethodID identity_id = env->GetStaticMethodID(
        hello_class, "jitIdentity", "(Ljava/lang/Object;)Ljava/lang/Object;");
    if (identity == nullptr || identity_id == nullptr) return 85;
    auto check_identity_jni = [&](jobject expected) {
      jobject actual = env->CallStaticObjectMethod(hello_class, identity_id, expected);
      const bool ok = !env->ExceptionCheck() && env->IsSameObject(actual, expected);
      if (actual != nullptr) env->DeleteLocalRef(actual);
      return ok;
    };
    // The first JNI calls exercise the normal interpreter entrypoint before
    // this method is admitted to the JIT. Keep both compressed null and
    // non-null returns in the same sequence used by the compiled checks.
    for (int iteration = 0; iteration < 8; ++iteration) {
      if (!check_identity_jni((iteration & 1) != 0 ? hello_class : nullptr)) return 86;
    }
    if (!jit->CompileMethod(identity, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(identity->GetEntryPointFromQuickCompiledCode())) return 85;
    for (int iteration = 0; iteration < 8; ++iteration) {
      art::mirror::Object* expected = (iteration & 1) != 0 ? hello.Get() : nullptr;
      uint32_t argument = art::mirror::CompressedReference<art::mirror::Object>::FromMirrorPtr(expected).AsVRegValue();
      art::JValue actual;
      identity->Invoke(self, &argument, sizeof(argument), &actual, "LL");
      if (self->IsExceptionPending() || actual.GetL() != expected) return 86;
    }
    for (int iteration = 0; iteration < 8; ++iteration) {
      if (!check_identity_jni((iteration & 1) != 0 ? hello_class : nullptr)) return 120;
    }
    // RemoveMethod reinitializes the entrypoint to the interpreter bridge,
    // giving this probe an explicit compiled -> interpreter return boundary.
    bool removed_identity = false;
    {
      art::ScopedThreadSuspension suspended(self, art::ThreadState::kSuspended);
      art::gc::ScopedGCCriticalSection gc(self, art::gc::kGcCauseInstrumentation,
                                        art::gc::kCollectorTypeInstrumentation);
      art::ScopedSuspendAll all("JIT identity removal acceptance");
      removed_identity = jit->GetCodeCache()->RemoveMethod(identity, /*release_memory=*/true);
    }
    const void* identity_entrypoint =
        identity->GetEntryPointFromQuickCompiledCode();
    const bool reinitialized_to_interpreter =
        class_linker->IsQuickToInterpreterBridge(identity_entrypoint) ||
        identity_entrypoint == art::interpreter::GetNterpEntryPoint();
    if (!removed_identity || !reinitialized_to_interpreter) {
      return 121;
    }
    for (int iteration = 0; iteration < 8; ++iteration) {
      if (!check_identity_jni((iteration & 1) != 0 ? hello_class : nullptr)) return 122;
    }
    std::cerr << "ART JIT acceptance: nullable identity interpreter/compiled/interpreter JNI PASS cycles=8\n";
    auto* native_identity = hello->FindClassMethod(
        "nativeReferenceIdentity", "(Ljava/lang/Object;)Ljava/lang/Object;",
        art::kRuntimePointerSize);
    if (native_identity == nullptr) return 124;
    for (int iteration = 0; iteration < 8; ++iteration) {
      auto* expected = (iteration & 1) != 0 ? hello.Get() : nullptr;
      uint32_t argument = art::mirror::CompressedReference<art::mirror::Object>::
          FromMirrorPtr(expected).AsVRegValue();
      art::JValue actual;
      native_identity->Invoke(self, &argument, sizeof(argument), &actual, "LL");
      if (self->IsExceptionPending() || actual.GetL() != expected) return 125;
    }
    std::cerr << "ART JIT acceptance: generic JNI native reference return PASS cycles=8\n";
    // Restore compiled execution before the existing concurrent-GC stress;
    // interpreter fallback must not accidentally satisfy that acceptance.
    if (!jit->CompileMethod(identity, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(identity->GetEntryPointFromQuickCompiledCode())) return 123;
    std::atomic<bool> gc_done{false};
    std::atomic<bool> gc_ok{false};
    std::thread gc_worker([&] {
      auto* runtime = art::Runtime::Current();
      if (runtime->AttachCurrentThread("jit-gc-audit", true, nullptr, false)) {
        for (int cycle = 0; cycle < 3; ++cycle) runtime->GetHeap()->CollectGarbage(false);
        runtime->DetachCurrentThread();
        gc_ok.store(true);
      }
      gc_done.store(true);
    });
    bool identities_ok = true;
    uint64_t calls = 0;
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
    while (!gc_done.load() && std::chrono::steady_clock::now() < deadline) {
      // Tiny leaf methods have no internal suspend point. The native caller
      // must cooperate at call boundaries; this is not a moving-GC/spill test.
      self->CheckSuspend();
      const bool non_null = (++calls & 1) != 0;
      uint32_t argument = art::mirror::CompressedReference<art::mirror::Object>::FromMirrorPtr(
          non_null ? hello.Get() : nullptr).AsVRegValue();
      art::JValue actual;
      identity->Invoke(self, &argument, sizeof(argument), &actual, "LL");
      if (self->IsExceptionPending() || actual.GetL() != (non_null ? hello.Get() : nullptr)) {
        identities_ok = false;
        break;
      }
    }
    const bool completed_while_calling = gc_done.load();
    {
      art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
      gc_worker.join();
    }
    if (!identities_ok || !gc_ok.load() || !completed_while_calling || calls == 0) return 91;
    std::cerr << "ART JIT acceptance: concurrent CMS/reference calls PASS gc=3 calls=" << calls << "\n";
    auto* divide = hello->FindClassMethod("jitDivide", "(I)I", art::kRuntimePointerSize);
    if (divide == nullptr || !jit->CompileMethod(divide, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(divide->GetEntryPointFromQuickCompiledCode())) return 87;
    uint32_t divisor = 0;
    art::JValue quotient;
    divide->Invoke(self, &divisor, sizeof(divisor), &quotient, "II");
    if (!self->IsExceptionPending()) return 88;
    jthrowable thrown = env->ExceptionOccurred();
    env->ExceptionClear();
    jclass arithmetic_exception = env->FindClass("java/lang/ArithmeticException");
    if (arithmetic_exception == nullptr || !env->IsInstanceOf(thrown, arithmetic_exception)) return 89;
    env->DeleteLocalRef(thrown);
    env->DeleteLocalRef(arithmetic_exception);
    divisor = 2;
    divide->Invoke(self, &divisor, sizeof(divisor), &quotient, "II");
    if (self->IsExceptionPending() || quotient.GetI() != 21) return 90;
    std::cerr << "ART JIT acceptance: compiled division exception/recovery PASS\n";
    jobject holder = env->AllocObject(hello_class);
    jfieldID int_field = env->GetFieldID(hello_class, "jitIntField", "I");
    jfieldID ref_field = env->GetFieldID(hello_class, "jitReferenceField", "Ljava/lang/Object;");
    jfieldID large_int_field = env->GetFieldID(hello_class, "jitLargeIntField", "I");
    jfieldID large_ref_field = env->GetFieldID(hello_class, "jitLargeReferenceField", "Ljava/lang/Object;");
    jfieldID large_volatile_int_field = env->GetFieldID(hello_class, "jitLargeVolatileIntField", "I");
    jmethodID get_int = env->GetStaticMethodID(hello_class, "jitReadInt", "(Ldev/darwinart/probe/Hello;)I");
    jmethodID get_ref = env->GetStaticMethodID(hello_class, "jitReadReference", "(Ldev/darwinart/probe/Hello;)Ljava/lang/Object;");
    jmethodID get_large_int = env->GetStaticMethodID(hello_class, "jitReadLargeInt", "(Ldev/darwinart/probe/Hello;)I");
    jmethodID get_large_ref = env->GetStaticMethodID(hello_class, "jitReadLargeReference", "(Ldev/darwinart/probe/Hello;)Ljava/lang/Object;");
    jmethodID get_large_volatile_int = env->GetStaticMethodID(hello_class, "jitReadLargeVolatileInt", "(Ldev/darwinart/probe/Hello;)I");
    jmethodID get_large_int_twice = env->GetStaticMethodID(hello_class, "jitReadLargeIntTwice", "(Ldev/darwinart/probe/Hello;)I");
    jmethodID catch_null = env->GetStaticMethodID(hello_class, "jitCatchNullField", "()I");
    if (!holder || !int_field || !ref_field || !large_int_field || !large_ref_field ||
        !large_volatile_int_field || !get_int || !get_ref || !get_large_int ||
        !get_large_ref || !get_large_volatile_int || !get_large_int_twice || !catch_null) return 92;
    art::ArtField* large_int_art_field = art::jni::DecodeArtField(large_int_field);
    art::ArtField* large_ref_art_field = art::jni::DecodeArtField(large_ref_field);
    if (large_int_art_field == nullptr || large_ref_art_field == nullptr ||
        large_int_art_field->GetOffset().Uint32Value() <= 16384u ||
        large_ref_art_field->GetOffset().Uint32Value() <= 16384u) {
      std::cerr << "ART JIT acceptance: generated large field offset is too small\n";
      return 100;
    }
    env->SetIntField(holder, int_field, 0x12345678);
    env->SetObjectField(holder, ref_field, hello_class);
    env->SetIntField(holder, large_int_field, 0x76543210);
    env->SetObjectField(holder, large_ref_field, hello_class);
    // Resolve field references through the interpreter before compiler admission.
    if (env->CallStaticIntMethod(hello_class, get_int, holder) != 0x12345678) return 93;
    jobject warm_reference = env->CallStaticObjectMethod(hello_class, get_ref, holder);
    if (!env->IsSameObject(warm_reference, hello_class)) return 94;
    env->DeleteLocalRef(warm_reference);
    auto* read_int = hello->FindClassMethod("jitReadInt", "(Ldev/darwinart/probe/Hello;)I", art::kRuntimePointerSize);
    auto* read_ref = hello->FindClassMethod("jitReadReference", "(Ldev/darwinart/probe/Hello;)Ljava/lang/Object;", art::kRuntimePointerSize);
    for (auto* getter : {read_int, read_ref}) {
      if (!getter || !jit->CompileMethod(getter, self, art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(getter->GetEntryPointFromQuickCompiledCode())) return 95;
    }
    auto* read_large_int = hello->FindClassMethod(
        "jitReadLargeInt", "(Ldev/darwinart/probe/Hello;)I", art::kRuntimePointerSize);
    auto* read_large_ref = hello->FindClassMethod(
        "jitReadLargeReference", "(Ldev/darwinart/probe/Hello;)Ljava/lang/Object;",
        art::kRuntimePointerSize);
    auto* read_large_volatile_int = hello->FindClassMethod(
        "jitReadLargeVolatileInt", "(Ldev/darwinart/probe/Hello;)I", art::kRuntimePointerSize);
    auto* read_large_int_twice = hello->FindClassMethod(
        "jitReadLargeIntTwice", "(Ldev/darwinart/probe/Hello;)I", art::kRuntimePointerSize);
    // Warm these distinct field references too; GetFieldID does not populate
    // the getter's DEX resolved-field slot. Negative volatile testing must not
    // pass merely because that slot has not been resolved yet.
    if (env->CallStaticIntMethod(hello_class, get_large_int, holder) != 0x76543210 ||
        env->ExceptionCheck()) return 102;
    if (static_cast<uint32_t>(env->CallStaticIntMethod(hello_class, get_large_int_twice, holder)) !=
        uint32_t(0x76543210u + 0x12345678u) || env->ExceptionCheck()) return 102;
    jobject warm_large_ref = env->CallStaticObjectMethod(hello_class, get_large_ref, holder);
    if (env->ExceptionCheck() || !env->IsSameObject(warm_large_ref, hello_class)) return 103;
    env->DeleteLocalRef(warm_large_ref);
    env->CallStaticIntMethod(hello_class, get_large_volatile_int, holder);
    env->CallStaticIntMethod(hello_class, get_large_int_twice, holder);
    if (env->ExceptionCheck()) return 104;
    if (read_large_int == nullptr || read_large_ref == nullptr ||
        read_large_volatile_int == nullptr || read_large_int_twice == nullptr ||
        !jit->CompileMethod(read_large_int, self, art::CompilationKind::kOptimized, false) ||
        !jit->CompileMethod(read_large_ref, self, art::CompilationKind::kOptimized, false) ||
        !jit->CompileMethod(read_large_volatile_int, self, art::CompilationKind::kOptimized, false) ||
        !jit->CompileMethod(read_large_int_twice, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(read_large_int_twice->GetEntryPointFromQuickCompiledCode()) ||
        !jit->GetCodeCache()->ContainsPc(read_large_int->GetEntryPointFromQuickCompiledCode()) ||
        !jit->GetCodeCache()->ContainsPc(read_large_ref->GetEntryPointFromQuickCompiledCode())) {
      std::cerr << "ART JIT acceptance: large field getter admission failed\n";
      return 101;
    }
    if (env->CallStaticIntMethod(hello_class, get_large_int, holder) != 0x76543210 ||
        env->ExceptionCheck()) return 102;
    jobject large_field_value = env->CallStaticObjectMethod(hello_class, get_large_ref, holder);
    if (env->ExceptionCheck() || !env->IsSameObject(large_field_value, hello_class)) return 103;
    env->DeleteLocalRef(large_field_value);
    std::cerr << "ART JIT acceptance: >16KiB int/ref/volatile field getters PASS\n";
    if (env->CallStaticIntMethod(hello_class, get_int, holder) != 0x12345678 || env->ExceptionCheck()) return 96;
    jobject field_value = env->CallStaticObjectMethod(hello_class, get_ref, holder);
    if (env->ExceptionCheck() || !env->IsSameObject(field_value, hello_class)) return 97;
    env->DeleteLocalRef(field_value);
    env->SetObjectField(holder, ref_field, nullptr);
    field_value = env->CallStaticObjectMethod(hello_class, get_ref, holder);
    if (field_value != nullptr || env->ExceptionCheck()) return 98;
    if (env->CallStaticIntMethod(hello_class, catch_null) != 42 || env->ExceptionCheck()) return 99;
    env->DeleteLocalRef(holder);
    std::cerr << "ART JIT acceptance: compiled int/ref fields and caller-caught NPE PASS\n";
    const char* invoke_names[] = {"jitCallInt", "jitCallRef", "jitCallMixed", "jitCallGc", "jitCallDivide"};
    const char* invoke_sigs[] = {"(II)I", "(Ljava/lang/Object;)Ljava/lang/Object;",
        "(ILjava/lang/Object;ILjava/lang/Object;I)Ljava/lang/Object;", "(Ljava/lang/Object;)Ljava/lang/Object;", "(I)I"};
    jmethodID invoke_ids[5];
    for (size_t i = 0; i < 5; ++i) {
      invoke_ids[i] = env->GetStaticMethodID(hello_class, invoke_names[i], invoke_sigs[i]);
      if (!invoke_ids[i]) return 106;
    }
    // Resolve call sites without first compiling their callees.
    env->CallStaticIntMethod(hello_class, invoke_ids[0], 17, 29);
    for (size_t i : {1u, 3u}) {
      jobject value = env->CallStaticObjectMethod(hello_class, invoke_ids[i], hello_class);
      env->DeleteLocalRef(value);
    }
    jobject mixed = env->CallStaticObjectMethod(hello_class, invoke_ids[2], 17, hello_class, 29, hello_class, 43);
    env->DeleteLocalRef(mixed);
    env->CallStaticIntMethod(hello_class, invoke_ids[4], 2);
    if (env->ExceptionCheck()) return 107;
    auto* ref_target = hello->FindClassMethod("jitCallRefTarget", invoke_sigs[1], art::kRuntimePointerSize);
    if (!ref_target || jit->GetCodeCache()->ContainsPc(ref_target->GetEntryPointFromQuickCompiledCode())) return 108;
    for (size_t i = 0; i < 5; ++i) {
      auto* wrapper = hello->FindClassMethod(invoke_names[i], invoke_sigs[i], art::kRuntimePointerSize);
      if (!wrapper || !jit->CompileMethod(wrapper, self, art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(wrapper->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT acceptance: invoke compilation failed " << invoke_names[i] << "\n";
        return 109;
      }
    }
    for (bool compiled_target : {false, true}) {
      if (compiled_target && !jit->CompileMethod(ref_target, self, art::CompilationKind::kOptimized, false)) return 110;
      for (int iteration = 0; iteration < 8; ++iteration) {
        jobject expected = (iteration & 1) != 0 ? hello_class : nullptr;
        jobject actual = env->CallStaticObjectMethod(hello_class, invoke_ids[1], expected);
        if (env->ExceptionCheck() || !env->IsSameObject(actual, expected)) return 111;
        if (actual != nullptr) env->DeleteLocalRef(actual);
      }
    }
    // A forwarding result alone also passes without inlining. Compare machine
    // call sites with the standalone identity and a non-inlineable GC target.
    auto count_link_branches = [](art::ArtMethod* method) {
      const auto* header = art::OatQuickMethodHeader::FromEntryPoint(
          method->GetEntryPointFromQuickCompiledCode());
      unsigned count = 0;
      for (size_t offset = 0; offset + 4 <= header->GetCodeSize(); offset += 4) {
        uint32_t instruction;
        std::memcpy(&instruction, header->GetCode() + offset, sizeof(instruction));
        if ((instruction & 0xfc000000u) == 0x94000000u ||
            (instruction & 0xfffffc1fu) == 0xd63f0000u) ++count;
      }
      return count;
    };
    auto* ref_wrapper = hello->FindClassMethod(invoke_names[1], invoke_sigs[1], art::kRuntimePointerSize);
    auto* gc_wrapper = hello->FindClassMethod(invoke_names[3], invoke_sigs[3], art::kRuntimePointerSize);
    unsigned identity_calls = count_link_branches(ref_target);
    unsigned wrapper_calls = count_link_branches(ref_wrapper);
    unsigned gc_calls = count_link_branches(gc_wrapper);
    std::cerr << "ART JIT inline identity: ARM64 link branches target=" << identity_calls
              << " wrapper=" << wrapper_calls << " noninline_gc=" << gc_calls << "\n";
    if (identity_calls != 0 || wrapper_calls != 0 || gc_calls == 0) return 119;
    jintArray inline_payload = env->NewIntArray(3);
    if (inline_payload == nullptr || env->ExceptionCheck()) return 119;
    const jint inline_markers[] = {17, -29, 0x12345678};
    env->SetIntArrayRegion(inline_payload, 0, 3, inline_markers);
    for (unsigned cycle = 0; cycle < 3; ++cycle) {
      {
        art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
        art::Runtime::Current()->GetHeap()->CollectGarbage(false);
      }
      jobject value = env->CallStaticObjectMethod(hello_class, invoke_ids[1], inline_payload);
      if (env->ExceptionCheck() || !env->IsSameObject(value, inline_payload)) return 119;
      jint markers[3] = {};
      env->GetIntArrayRegion(static_cast<jintArray>(value), 0, 3, markers);
      env->DeleteLocalRef(value);
      if (env->ExceptionCheck() || std::memcmp(markers, inline_markers, sizeof(markers)) != 0 ||
          !jit->GetCodeCache()->ContainsPc(ref_wrapper->GetEntryPointFromQuickCompiledCode())) return 119;
    }
    env->DeleteLocalRef(inline_payload);
    std::cerr << "ART JIT inline identity: call eliminated, nullable and heap payload across GC PASS\n";
    const char* range_signature = "(IIIIIIIJDLjava/lang/Object;)J";
    jmethodID range_id = env->GetStaticMethodID(hello_class, "jitRangeCall", range_signature);
    auto* range_method = hello->FindClassMethod("jitRangeCall", range_signature, art::kRuntimePointerSize);
    if (range_id == nullptr || range_method == nullptr || env->ExceptionCheck()) return 119;
    art::CodeItemDataAccessor range_code(*range_method->GetDexFile(), range_method->GetCodeItem());
    if (art::Instruction::At(range_code.Insns())->Opcode() != art::Instruction::INVOKE_STATIC_RANGE) return 119;
    jvalue range_args[10] = {};
    for (int i = 0; i < 7; ++i) range_args[i].i = (i + 1) * 11;
    range_args[7].j = 0x123456789abcdefLL;
    range_args[8].d = -1234.5;
    range_args[9].l = hello_class;
    if (env->CallStaticLongMethodA(hello_class, range_id, range_args) != range_args[7].j || env->ExceptionCheck()) return 119;
    if (!jit->CompileMethod(range_method, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(range_method->GetEntryPointFromQuickCompiledCode())) {
      std::cerr << "ART JIT range: compile failed\n";
      return 119;
    }
    for (unsigned cycle = 0; cycle < 3; ++cycle) {
      if (env->CallStaticLongMethodA(hello_class, range_id, range_args) != range_args[7].j || env->ExceptionCheck()) return 119;
      range_args[9].l = nullptr;
      if (env->CallStaticLongMethodA(hello_class, range_id, range_args) != -1 || env->ExceptionCheck()) return 119;
      range_args[9].l = hello_class;
    }
    for (unsigned slot = 0; slot < 7; ++slot) {
      range_args[slot].i += 1;
      if (env->CallStaticLongMethodA(hello_class, range_id, range_args) != -1 || env->ExceptionCheck()) return 119;
      range_args[slot].i -= 1;
    }
    range_args[8].d = 1234.5;
    if (env->CallStaticLongMethodA(hello_class, range_id, range_args) != -1 || env->ExceptionCheck()) return 119;
    range_args[8].d = -1234.5;
    range_args[7].j = -0x123456789abcdefLL;
    if (env->CallStaticLongMethodA(hello_class, range_id, range_args) != range_args[7].j || env->ExceptionCheck() ||
        !jit->GetCodeCache()->ContainsPc(range_method->GetEntryPointFromQuickCompiledCode())) return 119;
    std::cerr << "ART JIT range: mixed stack/register long/double/reference and callee GC PASS\n";
    jmethodID throw_id = env->GetStaticMethodID(hello_class, "jitThrow", "(Ljava/lang/Throwable;)V");
    jmethodID catch_id = env->GetStaticMethodID(hello_class, "jitCatchThrow", "(Ljava/lang/Throwable;)I");
    auto* throw_method = hello->FindClassMethod("jitThrow", "(Ljava/lang/Throwable;)V", art::kRuntimePointerSize);
    jclass exception_class = env->FindClass("java/lang/IllegalStateException");
    jmethodID exception_ctor = exception_class == nullptr ? nullptr : env->GetMethodID(exception_class, "<init>", "()V");
    if (!throw_id || !catch_id || !throw_method || !exception_ctor || env->ExceptionCheck()) return 119;
    jobject exception = env->NewObject(exception_class, exception_ctor);
    if (!exception || env->ExceptionCheck() || env->CallStaticIntMethod(hello_class, catch_id, exception) != 42 ||
        env->ExceptionCheck() || !jit->CompileMethod(throw_method, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(throw_method->GetEntryPointFromQuickCompiledCode())) return 119;
    for (int cycle = 0; cycle < 3; ++cycle) {
      env->CallStaticVoidMethod(hello_class, throw_id, exception);
      jthrowable caught = env->ExceptionOccurred();
      env->ExceptionClear();
      bool same = caught != nullptr && env->IsSameObject(caught, exception);
      env->DeleteLocalRef(caught);
      if (!same || env->CallStaticIntMethod(hello_class, catch_id, exception) != 42 || env->ExceptionCheck() ||
          env->CallStaticIntMethod(hello_class, catch_id, nullptr) != 41 || env->ExceptionCheck()) return 119;
      {
        art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
        art::Runtime::Current()->GetHeap()->CollectGarbage(false);
      }
    }
    if (!jit->GetCodeCache()->ContainsPc(throw_method->GetEntryPointFromQuickCompiledCode())) return 119;
    jmethodID catch_return_id = env->GetStaticMethodID(hello_class, "jitCatchReturn", "(Ljava/lang/Throwable;)Ljava/lang/Throwable;");
    auto* catch_method = hello->FindClassMethod("jitCatchReturn", "(Ljava/lang/Throwable;)Ljava/lang/Throwable;", art::kRuntimePointerSize);
    if (!catch_return_id || !catch_method || env->ExceptionCheck()) return 119;
    jobject warm_caught = env->CallStaticObjectMethod(hello_class, catch_return_id, exception);
    if (env->ExceptionCheck() || !env->IsSameObject(warm_caught, exception)) return 119;
    env->DeleteLocalRef(warm_caught);
    if (!jit->CompileMethod(catch_method, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(catch_method->GetEntryPointFromQuickCompiledCode())) {
      std::cerr << "ART JIT catch: compilation failed\n";
      return 119;
    }
    art::CodeInfo catch_info(art::OatQuickMethodHeader::FromEntryPoint(catch_method->GetEntryPointFromQuickCompiledCode()));
    bool has_catch_map = false;
    for (const auto& map : catch_info.GetStackMaps())
      has_catch_map |= map.GetKind() == art::StackMap::Kind::Catch;
    if (!has_catch_map) {
      std::cerr << "ART JIT catch: missing compiled catch stackmap\n";
      return 119;
    }
    jclass npe_class = env->FindClass("java/lang/NullPointerException");
    if (!npe_class || env->ExceptionCheck()) return 119;
    for (int cycle = 0; cycle < 3; ++cycle) {
      for (bool is_null : {false, true}) {
        jobject value = env->CallStaticObjectMethod(hello_class, catch_return_id, is_null ? nullptr : exception);
        if (env->ExceptionCheck() || value == nullptr ||
            (is_null ? !env->IsInstanceOf(value, npe_class) : !env->IsSameObject(value, exception))) return 119;
        env->DeleteLocalRef(value);
      }
      {
        art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
        art::Runtime::Current()->GetHeap()->CollectGarbage(false);
      }
    }
    if (!jit->GetCodeCache()->ContainsPc(catch_method->GetEntryPointFromQuickCompiledCode())) return 119;
    env->DeleteLocalRef(npe_class);
    std::cerr << "ART JIT catch: compiled handler exact object/null NPE/TLS clear/GC PASS\n";
    jmethodID typed_id = env->GetStaticMethodID(hello_class, "jitTypedCatch", "(Ljava/lang/Throwable;)Ljava/lang/Throwable;");
    auto* typed_method = hello->FindClassMethod("jitTypedCatch", "(Ljava/lang/Throwable;)Ljava/lang/Throwable;", art::kRuntimePointerSize);
    if (!typed_id || !typed_method || env->ExceptionCheck()) return 119;
    env->CallStaticObjectMethod(hello_class, typed_id, exception);
    if (env->ExceptionCheck() || !jit->CompileMethod(typed_method, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(typed_method->GetEntryPointFromQuickCompiledCode())) return 119;
    art::CodeInfo typed_info(art::OatQuickMethodHeader::FromEntryPoint(typed_method->GetEntryPointFromQuickCompiledCode()));
    unsigned typed_maps = 0;
    for (const auto& map : typed_info.GetStackMaps()) typed_maps += map.GetKind() == art::StackMap::Kind::Catch;
    if (typed_maps < 2) return 119;
    for (const char* type : {"java/lang/IllegalArgumentException", "java/lang/NumberFormatException",
                             "java/lang/IllegalStateException", "java/io/IOException"}) {
      jclass cls = env->FindClass(type);
      jmethodID ctor = cls == nullptr ? nullptr : env->GetMethodID(cls, "<init>", "()V");
      if (!ctor || env->ExceptionCheck()) return 119;
      jobject input = env->NewObject(cls, ctor);
      if (!input || env->ExceptionCheck()) return 119;
      for (unsigned cycle = 0; cycle < 3; ++cycle) {
        jobject output = env->CallStaticObjectMethod(hello_class, typed_id, input);
        bool propagate = std::strcmp(type, "java/io/IOException") == 0;
        bool null_result = std::strcmp(type, "java/lang/IllegalStateException") == 0;
        if (propagate) {
          jthrowable pending = env->ExceptionOccurred();
          env->ExceptionClear();
          bool same = pending != nullptr && env->IsSameObject(pending, input);
          env->DeleteLocalRef(pending);
          if (!same) return 119;
        } else if (env->ExceptionCheck() || (null_result ? output != nullptr : !env->IsSameObject(output, input))) return 119;
        env->DeleteLocalRef(output);
        {
          art::ScopedThreadStateChange native(self, art::ThreadState::kNative);
          art::Runtime::Current()->GetHeap()->CollectGarbage(false);
        }
      }
      env->DeleteLocalRef(input);
      env->DeleteLocalRef(cls);
    }
    env->CallStaticObjectMethod(hello_class, typed_id, nullptr);
    if (!ClearExpectedArrayException(env, "java/lang/NullPointerException") ||
        !jit->GetCodeCache()->ContainsPc(typed_method->GetEntryPointFromQuickCompiledCode())) return 119;
    std::cerr << "ART JIT typed catch: two handlers, subclass match, unmatched propagation and GC PASS\n";
    jmethodID finally_id = env->GetStaticMethodID(hello_class, "jitFinally", "(Ljava/lang/Throwable;)Ljava/lang/Throwable;");
    auto* finally_method = hello->FindClassMethod("jitFinally", "(Ljava/lang/Throwable;)Ljava/lang/Throwable;", art::kRuntimePointerSize);
    jfieldID finally_count = env->GetStaticFieldID(hello_class, "jitFinallyCount", "I");
    jfieldID finally_seen = env->GetStaticFieldID(hello_class, "jitFinallySeen", "Ljava/lang/Throwable;");
    if (!finally_id || !finally_method || !finally_count || !finally_seen || env->ExceptionCheck()) return 119;
    env->CallStaticObjectMethod(hello_class, finally_id, nullptr);
    if (env->ExceptionCheck() || !jit->CompileMethod(finally_method, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(finally_method->GetEntryPointFromQuickCompiledCode())) {
      std::cerr << "ART JIT finally: compile failed\n";
      return 119;
    }
    art::CodeInfo finally_info(art::OatQuickMethodHeader::FromEntryPoint(finally_method->GetEntryPointFromQuickCompiledCode()));
    bool finally_map = false;
    for (const auto& map : finally_info.GetStackMaps()) finally_map |= map.GetKind() == art::StackMap::Kind::Catch;
    if (!finally_map) return 119;
    for (unsigned cycle = 0; cycle < 3; ++cycle) {
      for (bool throwing : {false, true}) {
        env->SetStaticIntField(hello_class, finally_count, 0);
        env->SetStaticObjectField(hello_class, finally_seen, nullptr);
        jobject output = env->CallStaticObjectMethod(hello_class, finally_id, throwing ? exception : nullptr);
        if (throwing) {
          jthrowable pending = env->ExceptionOccurred();
          env->ExceptionClear();
          bool same = pending != nullptr && env->IsSameObject(pending, exception);
          env->DeleteLocalRef(pending);
          if (!same) return 119;
        } else if (env->ExceptionCheck() || output != nullptr) return 119;
        env->DeleteLocalRef(output);
        jobject seen = env->GetStaticObjectField(hello_class, finally_seen);
        bool same = env->IsSameObject(seen, throwing ? exception : nullptr);
        env->DeleteLocalRef(seen);
        if (!same || env->GetStaticIntField(hello_class, finally_count) != 1 || env->ExceptionCheck()) return 119;
      }
    }
    jfieldID replacement_field = env->GetStaticFieldID(hello_class, "jitFinallyReplacement", "Ljava/lang/Throwable;");
    jobject replacement = env->NewObject(exception_class, exception_ctor);
    if (!replacement_field || !replacement || env->ExceptionCheck()) return 119;
    env->SetStaticObjectField(hello_class, replacement_field, replacement);
    for (bool throwing : {false, true}) {
      env->SetStaticIntField(hello_class, finally_count, 0);
      env->CallStaticObjectMethod(hello_class, finally_id, throwing ? exception : nullptr);
      jthrowable pending = env->ExceptionOccurred();
      env->ExceptionClear();
      bool same = pending != nullptr && env->IsSameObject(pending, replacement);
      env->DeleteLocalRef(pending);
      if (!same || env->GetStaticIntField(hello_class, finally_count) != 1 || env->ExceptionCheck()) return 119;
    }
    env->SetStaticObjectField(hello_class, replacement_field, nullptr);
    env->DeleteLocalRef(replacement);
    env->SetStaticObjectField(hello_class, finally_seen, nullptr);
    if (!jit->GetCodeCache()->ContainsPc(finally_method->GetEntryPointFromQuickCompiledCode())) return 119;
    std::cerr << "ART JIT finally: cleanup exception supersedes normal return/original exception PASS\n";
    std::cerr << "ART JIT finally: normal/exception once-only cleanup, callee GC and exact rethrow PASS\n";
    jmethodID monitor_id = env->GetStaticMethodID(hello_class, "jitMonitor", "(Ljava/lang/Throwable;)Ljava/lang/Throwable;");
    auto* monitor_method = hello->FindClassMethod("jitMonitor", "(Ljava/lang/Throwable;)Ljava/lang/Throwable;", art::kRuntimePointerSize);
    jfieldID monitor_throw = env->GetStaticFieldID(hello_class, "jitMonitorThrow", "Z");
    jfieldID monitor_held = env->GetStaticFieldID(hello_class, "jitMonitorHeld", "Z");
    if (!monitor_id || !monitor_method || !monitor_throw || !monitor_held || env->ExceptionCheck()) return 119;
    jobject warm_monitor = env->CallStaticObjectMethod(hello_class, monitor_id, exception);
    if (env->ExceptionCheck() || !env->IsSameObject(warm_monitor, exception)) return 119;
    env->DeleteLocalRef(warm_monitor);
    if (!jit->CompileMethod(monitor_method, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(monitor_method->GetEntryPointFromQuickCompiledCode())) {
      std::cerr << "ART JIT monitor: compilation failed\n";
      return 119;
    }
    for (bool reentrant : {false, true}) {
      if (reentrant && env->MonitorEnter(exception) != JNI_OK) return 119;
      if (reentrant) {
        // Hash a held thin lock to force AOSP monitor inflation before the
        // compiled recursive enter/exit and exceptional cleanup.
        jclass system_class = env->FindClass("java/lang/System");
        jmethodID hash_id = system_class == nullptr ? nullptr : env->GetStaticMethodID(system_class, "identityHashCode", "(Ljava/lang/Object;)I");
        if (!hash_id || env->ExceptionCheck()) return 119;
        env->CallStaticIntMethod(system_class, hash_id, exception);
        env->DeleteLocalRef(system_class);
        if (env->ExceptionCheck()) return 119;
        if (self->DecodeJObject(exception)->GetLockWord(true).GetState() != art::LockWord::kFatLocked) return 119;
      }
      for (bool throwing : {false, true}) {
        env->SetStaticBooleanField(hello_class, monitor_throw, throwing);
        env->SetStaticBooleanField(hello_class, monitor_held, false);
        jobject value = env->CallStaticObjectMethod(hello_class, monitor_id, exception);
        if (throwing) {
          jthrowable pending = env->ExceptionOccurred();
          env->ExceptionClear();
          bool same = pending != nullptr && env->IsSameObject(pending, exception);
          env->DeleteLocalRef(pending);
          if (!same) return 119;
        } else if (env->ExceptionCheck() || !env->IsSameObject(value, exception)) return 119;
        env->DeleteLocalRef(value);
        if (!env->GetStaticBooleanField(hello_class, monitor_held) || env->ExceptionCheck() ||
            art::Monitor::GetLockOwnerThreadId(self->DecodeJObject(exception)) != (reentrant ? self->GetThreadId() : 0)) return 119;
      }
      if (reentrant && env->MonitorExit(exception) != JNI_OK) return 119;
      if (art::Monitor::GetLockOwnerThreadId(self->DecodeJObject(exception)) != 0) return 119;
    }
    env->SetStaticBooleanField(hello_class, monitor_throw, false);
    env->CallStaticObjectMethod(hello_class, monitor_id, nullptr);
    if (!ClearExpectedArrayException(env, "java/lang/NullPointerException") ||
        !jit->GetCodeCache()->ContainsPc(monitor_method->GetEntryPointFromQuickCompiledCode())) return 119;
    std::cerr << "ART JIT monitor: held during GC, normal/exception unlock, inflated recursion and null PASS\n";
    if (!CheckJitMonitorContention(env, self, hello_class, monitor_id, exception)) return 119;
    for (bool throwing : {false, true}) {
      jobject fresh_lock = env->NewObject(exception_class, exception_ctor);
      if (!fresh_lock || env->ExceptionCheck()) return 119;
      env->SetStaticBooleanField(hello_class, monitor_throw, throwing);
      bool ok = CheckJitMonitorContention(env, self, hello_class, monitor_id, fresh_lock, throwing, true);
      env->SetStaticBooleanField(hello_class, monitor_throw, false);
      env->DeleteLocalRef(fresh_lock);
      if (!ok) return 119;
    }
    env->DeleteLocalRef(exception);
    env->DeleteLocalRef(exception_class);
    std::cerr << "ART JIT throw: native boundary exact Throwable, Java catch, null/NPE and GC PASS\n";
    if (env->CallStaticIntMethod(hello_class, invoke_ids[0], 17, 29) != ((17 * 31 + 29) ^ (17 >> 29))) return 112;
    mixed = env->CallStaticObjectMethod(hello_class, invoke_ids[2], 17, hello_class, 29, hello_class, 43);
    if (env->ExceptionCheck() || !env->IsSameObject(mixed, hello_class)) return 113;
    env->DeleteLocalRef(mixed);
    jobject after_gc = env->CallStaticObjectMethod(hello_class, invoke_ids[3], hello_class);
    if (env->ExceptionCheck() || !env->IsSameObject(after_gc, hello_class)) return 114;
    env->DeleteLocalRef(after_gc);
    jmethodID catch_call = env->GetStaticMethodID(hello_class, "jitCatchCall", "()I");
    if (!catch_call || env->CallStaticIntMethod(hello_class, catch_call) != 42 || env->ExceptionCheck()) return 115;
    if (env->CallStaticIntMethod(hello_class, invoke_ids[4], 2) != 21 || env->ExceptionCheck()) return 116;
    jmethodID native_call = env->GetStaticMethodID(hello_class, "jitCallNative", "()I");
    jmethodID reordered_call = env->GetStaticMethodID(hello_class, "jitCallReordered", "(II)I");
    if (!native_call || !reordered_call) return 117;
    if (env->CallStaticIntMethod(hello_class, native_call) != 16384) return 117;
    env->CallStaticIntMethod(hello_class, reordered_call, 17, 29);
    if (env->ExceptionCheck()) return 117;
    if (!CheckJitNativeCalls(env, self, jit, hello, hello_class)) return 118;
    if (!CheckJitCallArguments(env, self, jit, hello, hello_class)) return 118;
    if (!CheckJitMixedArguments(env, self, jit, hello, hello_class)) return 118;
    if (!CheckJitComposedCalls(env, self, jit, hello, hello_class)) return 118;
    if (!CheckJitVirtualComposed(env, self, jit, hello, hello_class)) return 118;
    if (!CheckJitInstanceBody(env, self, jit, hello, hello_class)) return 118;
    if (!CheckJitFieldComposed(env, self, jit, hello, hello_class)) return 118;
    if (!CheckJitArrayComposed(env, self, jit, hello, hello_class)) return 118;
    if (!CheckJitArrayAllocationComposed(env, self, jit, hello, hello_class)) return 118;
    std::cerr << "ART JIT acceptance: managed calls interpreted/compiled nullable/mixed/GC/exception PASS "
                 "forwarding_cycles=8\n";
    auto* exit_identity = hello->FindClassMethod(
        "jitExitIdentity", "(Ljava/lang/Object;)Ljava/lang/Object;", art::kRuntimePointerSize);
    if (!CheckJitNumeric(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitBits(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitSpecializedIntrinsics(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitStringIntrinsics(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitHiddenStringIntrinsics(env, self, jit, hello)) return 119;
    if (!CheckJitSystemArrayCopy(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitMathHInvoke(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitCrc32(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitMemory(env, self, jit)) return 119;
    if (!CheckJitReference(env, self, jit)) return 119;
    if (!CheckJitBoxing(env, self, jit)) return 119;
    if (!CheckJitUnsafeIntrinsics(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitCompare(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitSwitch(env, self, jit, hello, hello_class)) return 119;
    {
      auto* first = hello->FindClassMethod("jitFirstInitialization", "()I", art::kRuntimePointerSize);
      jmethodID first_id = env->GetStaticMethodID(hello_class, "jitFirstInitialization", "()I");
      jfieldID count_id = env->GetStaticFieldID(hello_class, "jitInitializationCount", "I");
      if (!first || !first_id || !count_id || env->ExceptionCheck()) return 119;
      art::CodeItemDataAccessor code(*first->GetDexFile(), first->GetCodeItem());
      const auto* call = art::Instruction::At(code.Insns());
      if (call->Opcode() != art::Instruction::INVOKE_STATIC) return 119;
      auto* callee = first->GetDexCache()->GetResolvedMethod(call->VRegB_35c());
      if (!callee) callee = art::Runtime::Current()->GetClassLinker()->ResolveMethodId(call->VRegB_35c(), first);
      std::cerr << "ART JIT first initialization: resolved=" << (callee != nullptr)
                << " initialized=" << (callee && callee->GetDeclaringClass()->IsInitialized()) << "\n";
      std::cerr << "ART JIT first initialization: eligible="
                << art::jit::DarwinJitCanCompile(first, art::CompilationKind::kOptimized)
                << " cached=" << (first->GetDexCache()->GetResolvedMethod(call->VRegB_35c()) != nullptr)
                << " count=" << env->GetStaticIntField(hello_class, count_id) << "\n";
      if (!callee || callee->GetDeclaringClass()->IsInitialized() ||
          env->GetStaticIntField(hello_class, count_id) != 0) return 119;
      if (!jit->CompileMethod(first, self, art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(first->GetEntryPointFromQuickCompiledCode())) return 119;
      std::cerr << "ART JIT first initialization: compiled; initialized="
                << callee->GetDeclaringClass()->IsInitialized() << "\n";
      if (callee->GetDeclaringClass()->IsInitialized() ||
          env->GetStaticIntField(hello_class, count_id) != 0) return 119;
      for (int i = 0; i < 3; ++i) {
        if (env->CallStaticIntMethod(hello_class, first_id) != 42 || env->ExceptionCheck() ||
            env->GetStaticIntField(hello_class, count_id) != 1) return 119;
      }
      if (!callee->GetDeclaringClass()->IsInitialized()) return 119;
      std::cerr << "ART JIT first initialization: cold before/after compile, result=42 once-only=1 PASS\n";
    }
    if (!CheckJitColdTypes(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitFailedInitialization(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitFailedStatic(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitColdStatic(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitColdStaticWrite(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitUnresolvedStatic(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitUnresolvedInstance(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitClinitMovingGc(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitRecursiveInitialization(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitConcurrentInitialization(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitConcurrentInitialization(env, self, jit, hello, hello_class, true)) return 119;
    if (!CheckJitTypes(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitIntDiv(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitScalarLoops(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitOsrFrame(self, jit, hello)) return 119;
    if (!CheckJitOsrWide(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitAutomaticOsr(self, jit, hello)) return 119;
    if (!CheckJitOsrException(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitOsrReferences(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitOsrGc(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitOsrGc(env, self, jit, hello, hello_class, true)) return 119;
    if (!CheckJitOsrDeopt(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitArrays(env, self, jit, hello, hello_class)) return 133;
    if (!CheckJitAllocations(env, self, jit, hello, hello_class)) return 134;
    // Preserve the earlier compiled-caller/interpreted-constructor coverage
    // before compiling the general constructor itself.
    if (!CheckJitObjectComposed(env, self, jit, hello, hello_class)) return 118;
    if (!CheckJitHandlers(env, self, jit, hello, hello_class)) return 118;
    if (!CheckJitNestedFinally(env, self, jit, hello, hello_class)) return 118;
    if (!CheckJitSynchronized(env, self, jit, hello, hello_class)) return 118;
    if (!CheckJitInterface(env, self, jit, hello, hello_class)) return 118;
    if (!CheckJitUnresolvedCalls(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitSuper(env, self, jit, hello_class)) return 119;
    if (!CheckJitLargeMethod(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitArrayLiterals(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitPolymorphic(env, self, jit, hello, hello_class)) return 119;
    if (!CheckJitInvokeCustom(env, self, jit, hello)) return 119;
    std::cerr << "ART JIT acceptance: typed array length/get/set, covariance, null/bounds/type exceptions with GC PASS\n";
    if (!CheckJitFields(env, self, jit, hello, hello_class)) return 132;
    std::cerr << "ART JIT acceptance: 72 instance/static/volatile typed field accessors with GC PASS\n";
    for (const char* name : {"jitRootString", "jitRootClass", "jitRootOtherClass"}) {
      auto* root_method = hello->FindClassMethod(name, "()Ljava/lang/Object;", art::kRuntimePointerSize);
      jmethodID root_id = env->GetStaticMethodID(hello_class, name, "()Ljava/lang/Object;");
      if (root_method == nullptr || root_id == nullptr) return 127;
      jobject expected_root = env->CallStaticObjectMethod(hello_class, root_id);
      if (env->ExceptionCheck() || expected_root == nullptr) return 128;
      if (!jit->CompileMethod(root_method, self, art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(root_method->GetEntryPointFromQuickCompiledCode())) return 129;
      for (int cycle = 0; cycle < 3; ++cycle) {
        {
          art::ScopedThreadStateChange suspended(self, art::ThreadState::kNative);
          art::Runtime::Current()->GetHeap()->CollectGarbage(false);
        }
        jobject actual_root = env->CallStaticObjectMethod(hello_class, root_id);
        if (env->ExceptionCheck() || !env->IsSameObject(actual_root, expected_root)) return 130;
        env->DeleteLocalRef(actual_root);
        if (!jit->GetCodeCache()->ContainsPc(root_method->GetEntryPointFromQuickCompiledCode())) return 131;
      }
      env->DeleteLocalRef(expected_root);
    }
    std::cerr << "ART JIT acceptance: compiled string/class root loads with GC PASS\n";
    if (exit_identity == nullptr ||
        !CheckReferenceExitHooks(self, jit, exit_identity, native_identity, hello)) return 126;
    std::cerr << "ART JIT acceptance: compiled/native exit hooks with GC PASS\n";
  }

  art::ArtMethod* native_round_trip = hello->FindClassMethod(
      "nativeRoundTrip", "()I", art::kRuntimePointerSize);
  if (native_round_trip == nullptr) {
    std::cerr << "ART Darwin JNI: nativeRoundTrip()I lookup failed\n";
    return 11;
  }
  art::JValue native_result;
  native_round_trip->Invoke(self, /* args= */ nullptr, /* args_size= */ 0u,
                            &native_result, "I");
  if (self->IsExceptionPending()) {
    std::cerr << "ART Darwin JNI: nativeRoundTrip()I threw\n";
    return 12;
  }
  if (native_result.GetI() != 42) {
    std::cerr << "ART Darwin JNI: expected 42, got " << native_result.GetI()
              << "\n";
    return 13;
  }

  art::ArtMethod* native_stack_pcs = hello->FindClassMethod(
      "nativeStackPcsRoundTrip", "()I", art::kRuntimePointerSize);
  if (native_stack_pcs == nullptr) {
    std::cerr << "ART Darwin JNI PCS: nativeStackPcsRoundTrip()I lookup failed\n";
    return 34;
  }
  art::JValue native_stack_pcs_result;
  native_stack_pcs->Invoke(self, /* args= */ nullptr, /* args_size= */ 0u,
                           &native_stack_pcs_result, "I");
  if (self->IsExceptionPending() || native_stack_pcs_result.GetI() != 42) {
    std::cerr << "ART Darwin JNI PCS: packed stack argument matrix failed result="
              << native_stack_pcs_result.GetI() << "\n";
    return 35;
  }

  art::ArtMethod* runtime_native_arraycopy = hello->FindClassMethod(
      "runtimeNativeArraycopy", "()I", art::kRuntimePointerSize);
  if (runtime_native_arraycopy == nullptr) {
    std::cerr
        << "ART runtime native: runtimeNativeArraycopy()I lookup failed\n";
    return 14;
  }
  art::JValue arraycopy_result;
  runtime_native_arraycopy->Invoke(self, /* args= */ nullptr,
                                   /* args_size= */ 0u, &arraycopy_result, "I");
  if (self->IsExceptionPending()) {
    std::cerr << "ART runtime native: runtimeNativeArraycopy()I threw\n";
    return 15;
  }
  if (arraycopy_result.GetI() != 42) {
    std::cerr << "ART runtime native: expected 42, got "
              << arraycopy_result.GetI() << "\n";
    return 16;
  }

  jmethodID java_main =
      env->GetStaticMethodID(hello_class, "main", "([Ljava/lang/String;)V");
  jclass string_class = env->FindClass("java/lang/String");
  jobjectArray java_args = string_class == nullptr
                               ? nullptr
                               : env->NewObjectArray(1, string_class, nullptr);
  jstring message = env->NewStringUTF("Hello from Darwin ART main: 안녕");
  if (java_main == nullptr || string_class == nullptr || java_args == nullptr ||
      message == nullptr) {
    std::cerr << "ART Darwin launcher: main(String[]) setup failed\n";
    return 18;
  }
  env->SetObjectArrayElement(java_args, 0, message);
  env->CallStaticVoidMethod(hello_class, java_main, java_args);
  env->DeleteLocalRef(message);
  env->DeleteLocalRef(java_args);
  env->DeleteLocalRef(string_class);
  if (env->ExceptionCheck()) {
    std::cerr << "ART Darwin launcher: main(String[]) threw\n"
              << self->GetException()->Dump() << "\n";
    env->ExceptionDescribe();
    return 19;
  }

  results->hello_answer = result.GetI();
  results->native_round_trip = native_result.GetI();
  results->arraycopy_result = arraycopy_result.GetI();
  return 0;
}

}  // namespace darwin_art_jni_acceptance_phase
