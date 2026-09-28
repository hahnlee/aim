#pragma once

#include <array>

namespace aim_jni_acceptance_phase {

inline bool CheckJitMemory(JNIEnv* env, art::Thread* self, art::jit::Jit* jit) {
  // Explicit tier-by-tier acceptance must not race the production hotness worker.
  art::jit::ScopedJitSuspend workers;
  constexpr const char* descriptor = "Llibcore/io/JitMemoryDirect;";
  art::ObjPtr<art::mirror::Class> mirror =
      art::Runtime::Current()->GetClassLinker()->FindSystemClass(self, descriptor);
  if (mirror == nullptr || self->IsExceptionPending()) return false;
  art::StackHandleScope<1> hs(self);
  art::Handle<art::mirror::Class> handle = hs.NewHandle(mirror);
  if (!art::Runtime::Current()->GetClassLinker()->EnsureInitialized(
          self, handle, true, true)) return false;
  art::Runtime::Current()->GetClassLinker()->MakeInitializedClassesVisiblyInitialized(
      self, /*wait=*/ true);
  jclass owner = self->GetJniEnv()->AddLocalReference<jclass>(mirror);
  struct MethodSpec { const char* name; const char* signature; };
  constexpr MethodSpec specs[] = {
      {"peekByte", "(J)B"}, {"peekShortNative", "(JZ)S"},
      {"peekIntNative", "(JZ)I"}, {"peekLongNative", "(JZ)J"},
      {"pokeByte", "(JB)V"}, {"pokeShortNative", "(JSZ)V"},
      {"pokeIntNative", "(JIZ)V"}, {"pokeLongNative", "(JJZ)V"},
  };
  std::array<art::ArtMethod*, std::size(specs)> methods{};
  std::array<jmethodID, std::size(specs)> ids{};
  for (size_t index = 0; index < std::size(specs); ++index) {
    methods[index] = mirror->FindClassMethod(
        specs[index].name, specs[index].signature, art::kRuntimePointerSize);
    ids[index] = env->GetStaticMethodID(owner, specs[index].name, specs[index].signature);
    if (methods[index] == nullptr || ids[index] == nullptr || env->ExceptionCheck()) return false;
  }

  constexpr jbyte byte_value = static_cast<jbyte>(0x81);
  constexpr jshort short_value = static_cast<jshort>(0x8123);
  constexpr jint int_value = static_cast<jint>(0x81234567u);
  constexpr jlong long_value = static_cast<jlong>(UINT64_C(0x8123456789abcdef));
  bool ok = true;
  for (int phase = 0; phase < 3 && ok; ++phase) {
    if (phase != 0) {
      art::CompilationKind kind = phase == 1
          ? art::CompilationKind::kBaseline
          : art::CompilationKind::kOptimized;
      for (size_t index = 0; index < methods.size(); ++index) {
        if (!jit->CompileMethod(methods[index], self, kind, false) ||
            !jit->GetCodeCache()->ContainsPc(methods[index]->GetEntryPointFromQuickCompiledCode())) {
          std::cerr << "ART JIT Memory compile failed phase=" << phase
                    << " method=" << specs[index].name << specs[index].signature << "\n";
          return false;
        }
      }
    }

    alignas(16) std::array<uint8_t, 32> bytes{};
    auto address = [&](size_t offset) {
      return static_cast<jlong>(reinterpret_cast<uintptr_t>(bytes.data() + offset));
    };
    jvalue byte_args[2]{};
    byte_args[0].j = address(0); byte_args[1].b = byte_value;
    env->CallStaticVoidMethodA(owner, ids[4], byte_args);
    jvalue short_args[3]{};
    short_args[0].j = address(1); short_args[1].s = short_value; short_args[2].z = JNI_FALSE;
    env->CallStaticVoidMethodA(owner, ids[5], short_args);
    jvalue int_args[3]{};
    int_args[0].j = address(5); int_args[1].i = int_value; int_args[2].z = JNI_FALSE;
    env->CallStaticVoidMethodA(owner, ids[6], int_args);
    jvalue long_args[3]{};
    long_args[0].j = address(11); long_args[1].j = long_value; long_args[2].z = JNI_FALSE;
    env->CallStaticVoidMethodA(owner, ids[7], long_args);

    jvalue peek[2]{};
    peek[0].j = address(0);
    ok &= env->CallStaticByteMethodA(owner, ids[0], peek) == byte_value;
    peek[0].j = address(1);
    peek[1].z = JNI_FALSE;
    ok &= env->CallStaticShortMethodA(owner, ids[1], peek) == short_value;
    peek[0].j = address(5);
    ok &= env->CallStaticIntMethodA(owner, ids[2], peek) == int_value;
    peek[0].j = address(11);
    ok &= env->CallStaticLongMethodA(owner, ids[3], peek) == long_value;
    jshort stored_short;
    jint stored_int;
    jlong stored_long;
    std::memcpy(&stored_short, bytes.data() + 1, sizeof(stored_short));
    std::memcpy(&stored_int, bytes.data() + 5, sizeof(stored_int));
    std::memcpy(&stored_long, bytes.data() + 11, sizeof(stored_long));
    ok &= bytes[0] == static_cast<uint8_t>(byte_value);
    ok &= stored_short == short_value && stored_int == int_value && stored_long == long_value;
    ok &= !env->ExceptionCheck();
    if (!ok) {
      std::cerr << "ART JIT Memory mismatch phase=" << phase
                << " exception=" << env->ExceptionCheck() << "\n";
      env->ExceptionDescribe();
    }
  }
  env->DeleteLocalRef(owner);
  if (!ok || env->ExceptionCheck()) return false;
  std::cerr << "ART JIT Memory: 8 peek/poke byte/short/int/long native-address intrinsics "
               "interpreter+baseline+optimized unaligned round trips PASS\n";
  return true;
}

}  // namespace aim_jni_acceptance_phase
