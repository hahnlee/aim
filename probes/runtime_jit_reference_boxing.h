#pragma once

#include <array>

namespace aim_jni_acceptance_phase {

inline bool CompileJitMethods(art::jit::Jit* jit,
                              art::Thread* self,
                              const std::vector<art::ArtMethod*>& methods,
                              art::CompilationKind kind,
                              const char* family) {
  for (art::ArtMethod* method : methods) {
    if (!jit->CompileMethod(method, self, kind, false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
      std::cerr << "ART JIT " << family << " compile failed method="
                << method->PrettyMethod() << "\n";
      return false;
    }
  }
  return true;
}

inline bool CheckJitReference(JNIEnv* env, art::Thread* self, art::jit::Jit* jit) {
  art::jit::ScopedJitSuspend workers;
  constexpr const char* descriptor = "Ljava/lang/ref/JitReferenceDirect;";
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
      {"getReferent", "(Ljava/lang/ref/Reference;)Ljava/lang/Object;"},
      {"refersTo", "(Ljava/lang/ref/Reference;Ljava/lang/Object;)Z"},
      {"reachabilityFence", "(Ljava/lang/Object;)V"},
  };
  std::vector<art::ArtMethod*> methods;
  std::array<jmethodID, std::size(specs)> ids{};
  for (size_t i = 0; i < std::size(specs); ++i) {
    art::ArtMethod* method = mirror->FindClassMethod(
        specs[i].name, specs[i].signature, art::kRuntimePointerSize);
    ids[i] = env->GetStaticMethodID(owner, specs[i].name, specs[i].signature);
    if (method == nullptr || ids[i] == nullptr || env->ExceptionCheck()) return false;
    methods.push_back(method);
  }
  art::ObjPtr<art::mirror::Class> reference_mirror =
      art::Runtime::Current()->GetClassLinker()->FindSystemClass(
          self, "Ljava/lang/ref/Reference;");
  art::ArtMethod* reference_get = reference_mirror == nullptr
      ? nullptr
      : reference_mirror->FindClassMethod(
            "get", "()Ljava/lang/Object;", art::kRuntimePointerSize);
  if (reference_get == nullptr || self->IsExceptionPending()) return false;
  methods.push_back(reference_get);

  jclass object_class = env->FindClass("java/lang/Object");
  jclass weak_class = env->FindClass("java/lang/ref/WeakReference");
  jmethodID object_init = env->GetMethodID(object_class, "<init>", "()V");
  jmethodID weak_init = env->GetMethodID(weak_class, "<init>", "(Ljava/lang/Object;)V");
  if (object_init == nullptr || weak_init == nullptr || env->ExceptionCheck()) return false;

  bool ok = true;
  for (int phase = 0; phase < 3 && ok; ++phase) {
    if (phase != 0 && !CompileJitMethods(
            jit, self, methods,
            phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized,
            "Reference")) {
      return false;
    }
    jobject payload = env->NewObject(object_class, object_init);
    jobject weak = env->NewObject(weak_class, weak_init, payload);
    jobject empty = env->NewObject(weak_class, weak_init, nullptr);
    jobject referent = env->CallStaticObjectMethod(owner, ids[0], weak);
    ok &= env->IsSameObject(referent, payload);
    ok &= env->CallStaticBooleanMethod(owner, ids[1], weak, payload) == JNI_TRUE;
    ok &= env->CallStaticBooleanMethod(owner, ids[1], weak, object_class) == JNI_FALSE;
    ok &= env->CallStaticBooleanMethod(owner, ids[1], empty, nullptr) == JNI_TRUE;
    env->CallStaticVoidMethod(owner, ids[2], payload);
    env->CallStaticVoidMethod(owner, ids[2], nullptr);
    art::Runtime::Current()->GetHeap()->CollectGarbage(false);
    jobject moved = env->CallStaticObjectMethod(owner, ids[0], weak);
    ok &= env->IsSameObject(moved, payload);
    ok &= env->CallStaticBooleanMethod(owner, ids[1], weak, payload) == JNI_TRUE;
    ok &= !env->ExceptionCheck();
    env->DeleteLocalRef(moved);
    env->DeleteLocalRef(referent);
    env->DeleteLocalRef(empty);
    env->DeleteLocalRef(weak);
    env->DeleteLocalRef(payload);
  }
  env->DeleteLocalRef(weak_class);
  env->DeleteLocalRef(object_class);
  env->DeleteLocalRef(owner);
  if (!ok || env->ExceptionCheck()) return false;
  std::cerr << "ART JIT Reference: getReferent/refersTo/reachabilityFence "
               "interpreter+baseline+optimized moving-GC identity/null PASS\n";
  return true;
}

inline bool CheckJitBoxing(JNIEnv* env, art::Thread* self, art::jit::Jit* jit) {
  art::jit::ScopedJitSuspend workers;
  constexpr const char* descriptor = "Ljava/lang/JitBoxingDirect;";
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
  struct BoxSpec {
    const char* wrapper_name;
    const char* wrapper_signature;
    const char* class_name;
    const char* unbox_name;
    const char* unbox_signature;
    jint cached;
    jint uncached;
  };
  constexpr BoxSpec specs[] = {
      {"byteValueOf", "(B)Ljava/lang/Byte;", "java/lang/Byte", "byteValue", "()B", -127, 127},
      {"shortValueOf", "(S)Ljava/lang/Short;", "java/lang/Short", "shortValue", "()S", -128, 0x1234},
      {"characterValueOf", "(C)Ljava/lang/Character;", "java/lang/Character", "charValue", "()C", 127, 0x1234},
      {"integerValueOf", "(I)Ljava/lang/Integer;", "java/lang/Integer", "intValue", "()I", -128, 0x12345678},
  };
  std::vector<art::ArtMethod*> methods;
  std::array<jmethodID, std::size(specs)> box_ids{};
  std::array<jmethodID, std::size(specs)> unbox_ids{};
  std::array<jclass, std::size(specs)> box_classes{};
  for (size_t i = 0; i < std::size(specs); ++i) {
    art::ArtMethod* method = mirror->FindClassMethod(
        specs[i].wrapper_name, specs[i].wrapper_signature, art::kRuntimePointerSize);
    box_ids[i] = env->GetStaticMethodID(owner, specs[i].wrapper_name, specs[i].wrapper_signature);
    box_classes[i] = env->FindClass(specs[i].class_name);
    unbox_ids[i] = env->GetMethodID(
        box_classes[i], specs[i].unbox_name, specs[i].unbox_signature);
    if (method == nullptr || box_ids[i] == nullptr || box_classes[i] == nullptr ||
        unbox_ids[i] == nullptr || env->ExceptionCheck()) return false;
    methods.push_back(method);
  }

  bool ok = true;
  for (int phase = 0; phase < 3 && ok; ++phase) {
    if (phase != 0 && !CompileJitMethods(
            jit, self, methods,
            phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized,
            "boxing")) {
      return false;
    }
    for (size_t i = 0; i < std::size(specs); ++i) {
      jvalue cached_arg{};
      jvalue uncached_arg{};
      if (i == 0) { cached_arg.b = static_cast<jbyte>(specs[i].cached); uncached_arg.b = static_cast<jbyte>(specs[i].uncached); }
      if (i == 1) { cached_arg.s = static_cast<jshort>(specs[i].cached); uncached_arg.s = static_cast<jshort>(specs[i].uncached); }
      if (i == 2) { cached_arg.c = static_cast<jchar>(specs[i].cached); uncached_arg.c = static_cast<jchar>(specs[i].uncached); }
      if (i == 3) { cached_arg.i = specs[i].cached; uncached_arg.i = specs[i].uncached; }
      jobject cached_a = env->CallStaticObjectMethodA(owner, box_ids[i], &cached_arg);
      jobject cached_b = env->CallStaticObjectMethodA(owner, box_ids[i], &cached_arg);
      jobject uncached_a = env->CallStaticObjectMethodA(owner, box_ids[i], &uncached_arg);
      jobject uncached_b = env->CallStaticObjectMethodA(owner, box_ids[i], &uncached_arg);
      ok &= cached_a != nullptr && uncached_a != nullptr;
      ok &= env->IsSameObject(cached_a, cached_b);
      if (i != 0) ok &= !env->IsSameObject(uncached_a, uncached_b);
      jint cached_value = 0;
      jint uncached_value = 0;
      if (i == 0) { cached_value = env->CallByteMethod(cached_a, unbox_ids[i]); uncached_value = env->CallByteMethod(uncached_a, unbox_ids[i]); }
      if (i == 1) { cached_value = env->CallShortMethod(cached_a, unbox_ids[i]); uncached_value = env->CallShortMethod(uncached_a, unbox_ids[i]); }
      if (i == 2) { cached_value = env->CallCharMethod(cached_a, unbox_ids[i]); uncached_value = env->CallCharMethod(uncached_a, unbox_ids[i]); }
      if (i == 3) { cached_value = env->CallIntMethod(cached_a, unbox_ids[i]); uncached_value = env->CallIntMethod(uncached_a, unbox_ids[i]); }
      ok &= cached_value == specs[i].cached && uncached_value == specs[i].uncached;
      art::Runtime::Current()->GetHeap()->CollectGarbage(false);
      ok &= env->IsInstanceOf(cached_a, box_classes[i]) && env->IsInstanceOf(uncached_a, box_classes[i]);
      ok &= !env->ExceptionCheck();
      env->DeleteLocalRef(uncached_b);
      env->DeleteLocalRef(uncached_a);
      env->DeleteLocalRef(cached_b);
      env->DeleteLocalRef(cached_a);
    }
  }
  for (jclass box_class : box_classes) env->DeleteLocalRef(box_class);
  env->DeleteLocalRef(owner);
  if (!ok || env->ExceptionCheck()) return false;
  std::cerr << "ART JIT boxing: Byte/Short/Character/Integer valueOf "
               "interpreter+baseline+optimized cache/allocation/moving-GC PASS\n";
  return true;
}

}  // namespace aim_jni_acceptance_phase
