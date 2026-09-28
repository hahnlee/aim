#pragma once

#include <array>

namespace aim_jni_acceptance_phase {

inline bool CheckJitUnsafeIntrinsics(JNIEnv* env,
                                     art::Thread* self,
                                     art::jit::Jit* jit,
                                     art::Handle<art::mirror::Class> app_owner,
                                     jclass java_owner) {
  art::StackHandleScope<2> hs(self);
  art::Handle<art::mirror::ClassLoader> app_loader = hs.NewHandle(app_owner->GetClassLoader());
  constexpr const char* fixture_descriptor = "Ldev/aim/probe/JitUnsafe;";
  art::ObjPtr<art::mirror::Class> fixture_mirror =
      art::Runtime::Current()->GetClassLinker()->FindClass(
          self, fixture_descriptor, std::char_traits<char>::length(fixture_descriptor), app_loader);
  if (fixture_mirror == nullptr || self->IsExceptionPending()) return false;
  art::Handle<art::mirror::Class> fixture_handle = hs.NewHandle(fixture_mirror);
  if (!art::Runtime::Current()->GetClassLinker()->EnsureInitialized(
          self, fixture_handle, true, true)) return false;
  // ART publishes initialized classes asynchronously. The production JIT asks
  // ClassLinker for this transition from NotifyCompilationOf(); this explicit
  // compiler test waits so its first requested compilation is deterministic.
  art::Runtime::Current()->GetClassLinker()->MakeInitializedClassesVisiblyInitialized(
      self, /*wait=*/ true);
  jclass fixture = self->GetJniEnv()->AddLocalReference<jclass>(fixture_mirror);
  jclass unsafe_class = env->FindClass("jdk/internal/misc/Unsafe");
  jmethodID get_unsafe = unsafe_class == nullptr
      ? nullptr
      : env->GetStaticMethodID(
            unsafe_class, "getUnsafe", "()Ljdk/internal/misc/Unsafe;");
  jobject unsafe = get_unsafe == nullptr
      ? nullptr
      : env->CallStaticObjectMethod(unsafe_class, get_unsafe);
  jobject holder = env->AllocObject(java_owner);
  jobject first_reference = env->AllocObject(java_owner);
  jobject second_reference = env->AllocObject(java_owner);
  if (fixture == nullptr || unsafe_class == nullptr || unsafe == nullptr || holder == nullptr ||
      first_reference == nullptr || second_reference == nullptr || env->ExceptionCheck()) {
    return false;
  }

  art::ArtField* int_field = app_owner->FindDeclaredInstanceField("jitIntField", "I");
  art::ArtField* long_field = app_owner->FindDeclaredInstanceField("jitFieldIJ", "J");
  art::ArtField* byte_field = app_owner->FindDeclaredInstanceField("jitFieldIB", "B");
  art::ArtField* reference_field =
      app_owner->FindDeclaredInstanceField("jitReferenceField", "Ljava/lang/Object;");
  if (int_field == nullptr || long_field == nullptr || byte_field == nullptr ||
      reference_field == nullptr) return false;
  const jlong int_offset = int_field->GetOffset().Uint32Value();
  const jlong long_offset = long_field->GetOffset().Uint32Value();
  const jlong byte_offset = byte_field->GetOffset().Uint32Value();
  const jlong reference_offset = reference_field->GetOffset().Uint32Value();

  struct MethodSpec { const char* name; const char* signature; };
  const MethodSpec specs[] = {
      {"getInt", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;J)I"},
      {"getIntAbsolute", "(Ljdk/internal/misc/Unsafe;J)I"},
      {"getIntVolatile", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;J)I"},
      {"getIntAcquire", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;J)I"},
      {"getLong", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;J)J"},
      {"getLongVolatile", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;J)J"},
      {"getLongAcquire", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;J)J"},
      {"getByte", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;J)B"},
      {"getReference", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;J)Ljava/lang/Object;"},
      {"getReferenceVolatile", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;J)Ljava/lang/Object;"},
      {"getReferenceAcquire", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;J)Ljava/lang/Object;"},
      {"putInt", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;JI)V"},
      {"putIntAbsolute", "(Ljdk/internal/misc/Unsafe;JI)V"},
      {"putIntRelease", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;JI)V"},
      {"putIntVolatile", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;JI)V"},
      {"putLong", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;JJ)V"},
      {"putLongRelease", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;JJ)V"},
      {"putLongVolatile", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;JJ)V"},
      {"putByte", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;JB)V"},
      {"putReference", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;JLjava/lang/Object;)V"},
      {"putReferenceRelease", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;JLjava/lang/Object;)V"},
      {"putReferenceVolatile", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;JLjava/lang/Object;)V"},
      {"compareAndSetInt", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;JII)Z"},
      {"compareAndSetLong", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;JJJ)Z"},
      {"compareAndSetReference", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;JLjava/lang/Object;Ljava/lang/Object;)Z"},
      {"getAndAddInt", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;JI)I"},
      {"getAndAddLong", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;JJ)J"},
      {"getAndSetInt", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;JI)I"},
      {"getAndSetLong", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;JJ)J"},
      {"getAndSetReference", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Object;JLjava/lang/Object;)Ljava/lang/Object;"},
      {"arrayBaseOffset", "(Ljdk/internal/misc/Unsafe;Ljava/lang/Class;)I"},
      {"loadFence", "(Ljdk/internal/misc/Unsafe;)V"},
      {"storeFence", "(Ljdk/internal/misc/Unsafe;)V"},
      {"fullFence", "(Ljdk/internal/misc/Unsafe;)V"},
  };
  static_assert(std::size(specs) == 34);
  std::array<jmethodID, std::size(specs)> ids{};
  std::array<art::ArtMethod*, std::size(specs)> methods{};
  for (size_t index = 0; index < std::size(specs); ++index) {
    ids[index] = env->GetStaticMethodID(fixture, specs[index].name, specs[index].signature);
    methods[index] = fixture_mirror->FindClassMethod(
        specs[index].name, specs[index].signature, art::kRuntimePointerSize);
    if (ids[index] == nullptr || methods[index] == nullptr || env->ExceptionCheck()) {
      std::cerr << "ART JIT Unsafe DEX method missing " << specs[index].name << "\n";
      return false;
    }
  }
  auto id = [&](const char* name) {
    for (size_t index = 0; index < std::size(specs); ++index) {
      if (std::strcmp(name, specs[index].name) == 0) return ids[index];
    }
    return jmethodID(nullptr);
  };

  jmethodID allocate = env->GetMethodID(unsafe_class, "allocateMemory", "(J)J");
  jmethodID free = env->GetMethodID(unsafe_class, "freeMemory", "(J)V");
  jclass ints = env->FindClass("[I");
  if (allocate == nullptr || free == nullptr || ints == nullptr || env->ExceptionCheck()) return false;
  jlong native_address = env->CallLongMethod(unsafe, allocate, jlong(16));
  if (native_address == 0 || env->ExceptionCheck()) return false;

  bool ok = true;
  for (int phase = 0; phase < 3 && ok; ++phase) {
    if (phase != 0) {
      art::CompilationKind kind = phase == 1
          ? art::CompilationKind::kBaseline
          : art::CompilationKind::kOptimized;
      for (size_t index = 0; index < methods.size(); ++index) {
        bool compiled = jit->CompileMethod(methods[index], self, kind, false);
        bool installed =
            jit->GetCodeCache()->ContainsPc(methods[index]->GetEntryPointFromQuickCompiledCode());
        if (!compiled || !installed) {
          std::cerr << "ART JIT Unsafe compile failed phase=" << phase
                    << " method=" << specs[index].name << " compiled=" << compiled
                    << " installed=" << installed
                    << " eligible=" << art::jit::DarwinJitCanCompile(methods[index], kind)
                    << " verified=" << methods[index]->GetDeclaringClass()->IsVerified()
                    << " visible=" << methods[index]->GetDeclaringClass()->IsVisiblyInitialized()
                    << " status="
                    << static_cast<int>(methods[index]->GetDeclaringClass()->GetStatus())
                    << " invokable=" << methods[index]->IsInvokable()
                    << " compilable=" << methods[index]->IsCompilable()
                    << " clinit=" << methods[index]->StillNeedsClinitCheck()
                    << " skip_checks=" << methods[index]->SkipAccessChecks() << "\n";
          ok = false;
          break;
        }
      }
    }
    if (!ok) break;

    env->CallStaticVoidMethod(fixture, id("putInt"), unsafe, holder, int_offset, 0x10203040);
    ok &= env->CallStaticIntMethod(fixture, id("getInt"), unsafe, holder, int_offset) == 0x10203040;
    env->CallStaticVoidMethod(fixture, id("putIntRelease"), unsafe, holder, int_offset, -73);
    ok &= env->CallStaticIntMethod(fixture, id("getIntAcquire"), unsafe, holder, int_offset) == -73;
    env->CallStaticVoidMethod(fixture, id("putIntVolatile"), unsafe, holder, int_offset, INT32_MIN);
    ok &= env->CallStaticIntMethod(fixture, id("getIntVolatile"), unsafe, holder, int_offset) == INT32_MIN;

    constexpr jlong wide = INT64_C(0x123456789abcdef);
    env->CallStaticVoidMethod(fixture, id("putLong"), unsafe, holder, long_offset, wide);
    ok &= env->CallStaticLongMethod(fixture, id("getLong"), unsafe, holder, long_offset) == wide;
    env->CallStaticVoidMethod(fixture, id("putLongRelease"), unsafe, holder, long_offset, -wide);
    ok &= env->CallStaticLongMethod(fixture, id("getLongAcquire"), unsafe, holder, long_offset) == -wide;
    env->CallStaticVoidMethod(fixture, id("putLongVolatile"), unsafe, holder, long_offset, INT64_MIN);
    ok &= env->CallStaticLongMethod(fixture, id("getLongVolatile"), unsafe, holder, long_offset) == INT64_MIN;
    env->CallStaticVoidMethod(fixture, id("putByte"), unsafe, holder, byte_offset, jint(-127));
    ok &= env->CallStaticByteMethod(fixture, id("getByte"), unsafe, holder, byte_offset) == -127;

    env->CallStaticVoidMethod(
        fixture, id("putReference"), unsafe, holder, reference_offset, first_reference);
    ok &= env->IsSameObject(
        env->CallStaticObjectMethod(fixture, id("getReference"), unsafe, holder, reference_offset),
        first_reference);
    env->CallStaticVoidMethod(
        fixture, id("putReferenceRelease"), unsafe, holder, reference_offset, second_reference);
    ok &= env->IsSameObject(
        env->CallStaticObjectMethod(fixture, id("getReferenceAcquire"), unsafe, holder, reference_offset),
        second_reference);
    env->CallStaticVoidMethod(
        fixture, id("putReferenceVolatile"), unsafe, holder, reference_offset, first_reference);
    {
      art::ScopedThreadStateChange suspended(self, art::ThreadState::kNative);
      art::Runtime::Current()->GetHeap()->CollectGarbage(false);
    }
    ok &= env->IsSameObject(
        env->CallStaticObjectMethod(
            fixture, id("getReferenceVolatile"), unsafe, holder, reference_offset),
        first_reference);

    env->CallStaticVoidMethod(fixture, id("putInt"), unsafe, holder, int_offset, 17);
    ok &= env->CallStaticBooleanMethod(
        fixture, id("compareAndSetInt"), unsafe, holder, int_offset, 17, 29);
    ok &= !env->CallStaticBooleanMethod(
        fixture, id("compareAndSetInt"), unsafe, holder, int_offset, 17, 31);
    ok &= env->CallStaticIntMethod(fixture, id("getAndAddInt"), unsafe, holder, int_offset, 13) == 29;
    ok &= env->CallStaticIntMethod(fixture, id("getAndSetInt"), unsafe, holder, int_offset, -11) == 42;

    env->CallStaticVoidMethod(fixture, id("putLong"), unsafe, holder, long_offset, jlong(101));
    ok &= env->CallStaticBooleanMethod(
        fixture, id("compareAndSetLong"), unsafe, holder, long_offset, jlong(101), jlong(211));
    ok &= env->CallStaticLongMethod(
        fixture, id("getAndAddLong"), unsafe, holder, long_offset, jlong(19)) == 211;
    ok &= env->CallStaticLongMethod(
        fixture, id("getAndSetLong"), unsafe, holder, long_offset, jlong(-7)) == 230;

    env->CallStaticVoidMethod(
        fixture, id("putReference"), unsafe, holder, reference_offset, first_reference);
    ok &= env->CallStaticBooleanMethod(fixture, id("compareAndSetReference"), unsafe, holder,
                                        reference_offset, first_reference, second_reference);
    jobject old = env->CallStaticObjectMethod(
        fixture, id("getAndSetReference"), unsafe, holder, reference_offset, first_reference);
    ok &= env->IsSameObject(old, second_reference);
    if (old != nullptr) env->DeleteLocalRef(old);

    env->CallStaticVoidMethod(
        fixture, id("putIntAbsolute"), unsafe, native_address, 0x55667788);
    ok &= env->CallStaticIntMethod(
        fixture, id("getIntAbsolute"), unsafe, native_address) == 0x55667788;
    ok &= env->CallStaticIntMethod(fixture, id("arrayBaseOffset"), unsafe, ints) > 0;
    env->CallStaticVoidMethod(fixture, id("loadFence"), unsafe);
    env->CallStaticVoidMethod(fixture, id("storeFence"), unsafe);
    env->CallStaticVoidMethod(fixture, id("fullFence"), unsafe);
    ok &= !env->ExceptionCheck();
  }

  env->CallVoidMethod(unsafe, free, native_address);
  env->DeleteLocalRef(ints);
  env->DeleteLocalRef(second_reference);
  env->DeleteLocalRef(first_reference);
  env->DeleteLocalRef(holder);
  env->DeleteLocalRef(unsafe);
  env->DeleteLocalRef(unsafe_class);
  env->DeleteLocalRef(fixture);
  if (!ok || env->ExceptionCheck()) return false;
  std::cerr << "ART JIT Unsafe intrinsics: 34 direct DEX read/write/CAS/update/fence/absolute "
               "interpreter+baseline+optimized with CC GC PASS\n";
  return true;
}

}  // namespace aim_jni_acceptance_phase
