#pragma once
#include <utility>
#include <vector>
#include "runtime_jit_loop_checkpoint.h"
#include "runtime_jit_virtual_acceptance.h"

namespace aim_jni_acceptance_phase {
inline bool CheckZeroArray(JNIEnv* env, jarray array, char type, jsize size) {
#define CHECK_ZERO_ARRAY(Code, Name, Element, Array) \
  case Code: { \
    std::vector<Element> values(size); \
    if (size != 0) env->Get##Name##ArrayRegion(static_cast<Array>(array), 0, size, values.data()); \
    if (env->ExceptionCheck()) return false; \
    const Element zero{}; \
    for (size_t i = 0; i != values.size(); ++i) if (std::memcmp(&values[i], &zero, sizeof(zero)) != 0) { \
      std::cerr << "ART JIT allocation nonzero: type=" << Code << " index=" << i \
                << " value=" << +values[i] << "\n"; return false; \
    } \
    return true; \
  }
  switch (type) {
    CHECK_ZERO_ARRAY('Z', Boolean, jboolean, jbooleanArray)
    CHECK_ZERO_ARRAY('B', Byte, jbyte, jbyteArray)
    CHECK_ZERO_ARRAY('C', Char, jchar, jcharArray)
    CHECK_ZERO_ARRAY('S', Short, jshort, jshortArray)
    CHECK_ZERO_ARRAY('I', Int, jint, jintArray)
    CHECK_ZERO_ARRAY('J', Long, jlong, jlongArray)
    CHECK_ZERO_ARRAY('F', Float, jfloat, jfloatArray)
    CHECK_ZERO_ARRAY('D', Double, jdouble, jdoubleArray)
    case 'L':
      for (jsize i = 0; i < size; ++i) {
        jobject value = env->GetObjectArrayElement(static_cast<jobjectArray>(array), i);
        if (value != nullptr) { env->DeleteLocalRef(value); return false; }
        if (env->ExceptionCheck()) return false;
      }
      return true;
    default: return false;
  }
#undef CHECK_ZERO_ARRAY
}

inline bool CheckJitAllocations(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                               art::Handle<art::mirror::Class> klass, jclass java_class) {
  if (!CheckJitVirtual(env, self, jit, klass, java_class, true) ||
      !CheckJitVirtual(env, self, jit, klass, java_class)) return false;
  for (char type : std::string_view("ZBCSIJFDL")) {
    const std::string name = std::string("jitNewArray") + type;
    const std::string descriptor = type == 'L' ? "[Ljava/lang/Object;" : std::string("[") + type;
    const std::string signature = "(I)" + descriptor;
    jmethodID id = env->GetStaticMethodID(java_class, name.c_str(), signature.c_str());
    auto* method = klass->FindClassMethod(name.c_str(), signature.c_str(), art::kRuntimePointerSize);
    jclass expected_class = env->FindClass(descriptor.c_str());
    if (id == nullptr || method == nullptr || expected_class == nullptr || env->ExceptionCheck())
      return false;
    jobject warm = env->CallStaticObjectMethod(java_class, id, 1);
    if (warm == nullptr || env->ExceptionCheck()) return false;
    env->DeleteLocalRef(warm);
    if (!jit->CompileMethod(method, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    for (jint size : {0, 1, 8193}) {
      jarray array = static_cast<jarray>(env->CallStaticObjectMethod(java_class, id, size));
      jarray second = static_cast<jarray>(env->CallStaticObjectMethod(java_class, id, size));
      if (array == nullptr || second == nullptr || env->ExceptionCheck() ||
          env->IsSameObject(array, second) || !env->IsInstanceOf(array, expected_class) ||
          env->GetArrayLength(array) != size || !CheckZeroArray(env, array, type, size)) return false;
      {
        art::ScopedThreadStateChange suspended(self, art::ThreadState::kNative);
        art::Runtime::Current()->GetHeap()->CollectGarbage(false);
      }
      if (env->GetArrayLength(array) != size || !CheckZeroArray(env, array, type, size) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
      env->DeleteLocalRef(second);
      env->DeleteLocalRef(array);
    }
    jobject negative = env->CallStaticObjectMethod(java_class, id, -1);
    if (negative != nullptr || !self->IsExceptionPending() ||
        !ClearExpectedArrayException(env, "java/lang/NegativeArraySizeException")) return false;
    env->DeleteLocalRef(expected_class);
    std::cerr << "ART JIT allocation: type=" << type << " lengths/zero/identity/negative/GC PASS\n";
  }
  // Custom class and array-valued component types use the same NEW_ARRAY ABI.
  for (const auto& fixture : {std::pair{"jitNewArrayCustom", "[Ldev/aim/probe/Hello;"},
                              std::pair{"jitNewArrayNested", "[[I"}}) {
    const std::string signature = std::string("(I)") + fixture.second;
    jmethodID id = env->GetStaticMethodID(java_class, fixture.first, signature.c_str());
    auto* method = klass->FindClassMethod(fixture.first, signature.c_str(), art::kRuntimePointerSize);
    if (id == nullptr || method == nullptr || env->ExceptionCheck()) return false;
    jobject warm = env->CallStaticObjectMethod(java_class, id, 1);
    if (warm == nullptr || env->ExceptionCheck()) return false;
    // This native-attached thread has no app-loader JNI caller. Use the
    // interpreter's class as the differential oracle, not boot FindClass.
    jclass expected = env->GetObjectClass(warm);
    if (expected == nullptr || env->ExceptionCheck()) return false;
    env->DeleteLocalRef(warm);
    if (!jit->CompileMethod(method, self, art::CompilationKind::kOptimized, false)) return false;
    jobjectArray array = static_cast<jobjectArray>(env->CallStaticObjectMethod(java_class, id, 37));
    if (array == nullptr || env->ExceptionCheck() || !env->IsInstanceOf(array, expected) ||
        env->GetArrayLength(array) != 37 || !CheckZeroArray(env, array, 'L', 37) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
    {
      art::ScopedThreadStateChange suspended(self, art::ThreadState::kNative);
      art::Runtime::Current()->GetHeap()->CollectGarbage(false);
    }
    if (!env->IsInstanceOf(array, expected) || !CheckZeroArray(env, array, 'L', 37)) return false;
    env->DeleteLocalRef(array);
    env->DeleteLocalRef(expected);
  }
  std::cerr << "ART JIT allocation: custom/nested component class roots PASS\n";
  {
    const char* signature = "(ILjava/lang/Object;)Ldev/aim/probe/Hello;";
    jmethodID id = env->GetStaticMethodID(java_class, "jitNewInstance", signature);
    auto* method = klass->FindClassMethod("jitNewInstance", signature, art::kRuntimePointerSize);
    auto* constructor = klass->FindClassMethod("<init>", "(ILjava/lang/Object;)V",
                                               art::kRuntimePointerSize);
    jfieldID marker = env->GetFieldID(java_class, "jitInstanceMarker", "I");
    jfieldID payload = env->GetFieldID(java_class, "jitInstancePayload", "Ljava/lang/Object;");
    if (id == nullptr || method == nullptr || constructor == nullptr ||
        marker == nullptr || payload == nullptr ||
        env->ExceptionCheck()) return false;
    if (jit->GetCodeCache()->ContainsPc(constructor->GetEntryPointFromQuickCompiledCode()))
      return false;
    jobject warm = env->CallStaticObjectMethod(java_class, id, 19, java_class);
    if (warm == nullptr || env->ExceptionCheck()) return false;
    jmethodID get_marker = env->GetMethodID(java_class, "jitGetMarker", "()I");
    jmethodID get_payload = env->GetMethodID(java_class, "jitGetPayload", "()Ljava/lang/Object;");
    if (get_marker == nullptr || get_payload == nullptr || env->ExceptionCheck() ||
        env->CallIntMethod(warm, get_marker) != 19) return false;
    jobject warm_payload = env->CallObjectMethod(warm, get_payload);
    if (env->ExceptionCheck() || !env->IsSameObject(warm_payload, java_class)) return false;
    env->DeleteLocalRef(warm_payload);
    std::vector<art::ArtMethod*> accessors;
    for (const auto& accessor : {std::pair{"jitGetMarker", "()I"},
                                std::pair{"jitSetMarker", "(I)V"},
                                std::pair{"jitGetPayload", "()Ljava/lang/Object;"},
                                std::pair{"jitSetPayload", "(Ljava/lang/Object;)V"}}) {
      auto* target = klass->FindClassMethod(accessor.first, accessor.second, art::kRuntimePointerSize);
      if (target == nullptr || target->IsStatic() ||
          !jit->CompileMethod(target, self, art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(target->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT instance accessor compile failed: " << accessor.first << "\n";
        return false;
      }
      accessors.push_back(target);
    }
    env->DeleteLocalRef(warm);
    if (!jit->CompileMethod(method, self, art::CompilationKind::kOptimized, false) ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
      std::cerr << "ART JIT allocation: new-instance compile failed\n";
      return false;
    }
    for (jint value : {0, 42, 0x1234567}) {
      const uint64_t collections_before = art::Runtime::Current()->GetHeap()->GetGcCount();
      jobject object = env->CallStaticObjectMethod(java_class, id, value, java_class);
      if (object == nullptr || env->ExceptionCheck() || !env->IsInstanceOf(object, java_class) ||
          env->GetIntField(object, marker) != value) return false;
      if (value == 42 && art::Runtime::Current()->GetHeap()->GetGcCount() <= collections_before)
        return false;
      {
        art::ScopedThreadStateChange suspended(self, art::ThreadState::kNative);
        art::Runtime::Current()->GetHeap()->CollectGarbage(false);
      }
      jobject actual_payload = env->GetObjectField(object, payload);
      jobject compiled_payload = env->CallObjectMethod(object, get_payload);
      if (env->ExceptionCheck() || !env->IsSameObject(actual_payload, java_class) ||
          !env->IsSameObject(compiled_payload, java_class) ||
          env->CallIntMethod(object, get_marker) != value ||
          env->GetIntField(object, marker) != value ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) return false;
      env->DeleteLocalRef(compiled_payload);
      env->DeleteLocalRef(actual_payload);
      env->DeleteLocalRef(object);
    }
    jobject failed = env->CallStaticObjectMethod(java_class, id, -1, nullptr);
    if (failed != nullptr || !ClearExpectedArrayException(env, "java/lang/IllegalArgumentException"))
      return false;
    jobject recovered = env->CallStaticObjectMethod(java_class, id, 7, nullptr);
    if (recovered == nullptr || env->ExceptionCheck() || env->GetIntField(recovered, marker) != 7 ||
        env->GetObjectField(recovered, payload) != nullptr ||
        !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode()) ||
        jit->GetCodeCache()->ContainsPc(constructor->GetEntryPointFromQuickCompiledCode())) return false;
    env->DeleteLocalRef(recovered);
    for (auto* target : accessors)
      if (!jit->GetCodeCache()->ContainsPc(target->GetEntryPointFromQuickCompiledCode())) return false;
    std::cerr << "ART JIT instance accessors: compiled receiver/get/set through constructor and JNI PASS\n";
    std::cerr << "ART JIT allocation: new-instance constructor GC/fields/roots/throw/recovery PASS\n";
    const char* empty_signature = "()Ldev/aim/probe/Hello;";
    jmethodID empty_id = env->GetStaticMethodID(java_class, "jitNewEmptyInstance", empty_signature);
    auto* empty_factory = klass->FindClassMethod("jitNewEmptyInstance", empty_signature,
                                               art::kRuntimePointerSize);
    auto* empty_constructor = klass->FindClassMethod("<init>", "()V", art::kRuntimePointerSize);
    if (empty_id == nullptr || empty_factory == nullptr || empty_constructor == nullptr ||
        env->ExceptionCheck()) return false;
    jobject empty_warm = env->CallStaticObjectMethod(java_class, empty_id);
    if (empty_warm == nullptr || env->ExceptionCheck()) return false;
    env->DeleteLocalRef(empty_warm);
    for (auto* target : {empty_constructor, empty_factory}) {
      if (!jit->CompileMethod(target, self, art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(target->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT empty constructor/factory compile failed: "
                  << target->PrettyMethod() << "\n";
        return false;
      }
    }
    jfieldID parent_marker = env->GetFieldID(java_class, "jitParentMarker", "I");
    jfieldID parent_receiver = env->GetFieldID(java_class, "jitParentReceiver", "Ljava/lang/Object;");
    jfieldID parent_throw = env->GetStaticFieldID(java_class, "jitParentThrow", "Z");
    if (parent_marker == nullptr || parent_receiver == nullptr || parent_throw == nullptr ||
        env->ExceptionCheck()) return false;
    const uint64_t parent_gc_before = art::Runtime::Current()->GetHeap()->GetGcCount();
    jobject empty = env->CallStaticObjectMethod(java_class, empty_id);
    if (empty == nullptr || env->ExceptionCheck() || !env->IsInstanceOf(empty, java_class) ||
        env->CallIntMethod(empty, get_marker) != 0 ||
        env->CallObjectMethod(empty, get_payload) != nullptr) return false;
    jobject parent_root = env->GetObjectField(empty, parent_receiver);
    if (env->ExceptionCheck() || !env->IsSameObject(parent_root, empty) ||
        env->GetIntField(empty, parent_marker) != 29 ||
        art::Runtime::Current()->GetHeap()->GetGcCount() <= parent_gc_before) return false;
    env->DeleteLocalRef(parent_root);
    {
      art::ScopedThreadStateChange suspended(self, art::ThreadState::kNative);
      art::Runtime::Current()->GetHeap()->CollectGarbage(false);
    }
    if (!env->IsInstanceOf(empty, java_class) ||
        !jit->GetCodeCache()->ContainsPc(empty_constructor->GetEntryPointFromQuickCompiledCode()) ||
        !jit->GetCodeCache()->ContainsPc(empty_factory->GetEntryPointFromQuickCompiledCode()))
      return false;
    env->DeleteLocalRef(empty);
    env->SetStaticBooleanField(java_class, parent_throw, JNI_TRUE);
    jobject parent_failed = env->CallStaticObjectMethod(java_class, empty_id);
    if (parent_failed != nullptr ||
        !ClearExpectedArrayException(env, "java/lang/IllegalArgumentException")) return false;
    env->SetStaticBooleanField(java_class, parent_throw, JNI_FALSE);
    jobject parent_recovered = env->CallStaticObjectMethod(java_class, empty_id);
    if (parent_recovered == nullptr || env->ExceptionCheck() ||
        env->GetIntField(parent_recovered, parent_marker) != 29 ||
        !jit->GetCodeCache()->ContainsPc(empty_constructor->GetEntryPointFromQuickCompiledCode()) ||
        !jit->GetCodeCache()->ContainsPc(empty_factory->GetEntryPointFromQuickCompiledCode())) return false;
    env->DeleteLocalRef(parent_recovered);
    std::cerr << "ART JIT constructor: parent GC/receiver root/throw/recovery PASS\n";
    std::cerr << "ART JIT allocation: compiled factory -> compiled empty constructor PASS\n";
    const char* wide_signature = "(JD)Ldev/aim/probe/Hello;";
    jmethodID wide_id = env->GetStaticMethodID(java_class, "jitNewWideInstance", wide_signature);
    auto* wide_factory = klass->FindClassMethod("jitNewWideInstance", wide_signature,
                                              art::kRuntimePointerSize);
    auto* wide_constructor = klass->FindClassMethod("<init>", "(JD)V", art::kRuntimePointerSize);
    jfieldID wide_field = env->GetFieldID(java_class, "jitInstanceWide", "J");
    jfieldID floating_field = env->GetFieldID(java_class, "jitInstanceFloating", "D");
    if (wide_id == nullptr || wide_factory == nullptr || wide_constructor == nullptr ||
        wide_field == nullptr || floating_field == nullptr || env->ExceptionCheck()) return false;
    jobject wide_warm = env->CallStaticObjectMethod(java_class, wide_id, jlong{1}, jdouble{2.5});
    if (wide_warm == nullptr || env->ExceptionCheck()) return false;
    env->DeleteLocalRef(wide_warm);
    for (auto* target : {wide_constructor, wide_factory}) {
      if (!jit->CompileMethod(target, self, art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(target->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT wide constructor/factory compile failed: " << target->PrettyMethod() << "\n";
        art::CodeItemDataAccessor failed_code(*target->GetDexFile(), target->GetCodeItem());
        for (auto pair : failed_code) std::cerr << pair.DexPc() << ": " << pair.Inst().DumpString(target->GetDexFile()) << "\n";
        return false;
      }
    }
    for (const auto& values : {std::pair{jlong{0x123456789abcdef}, jdouble{-12345.125}},
                               std::pair{jlong{-1234567890123}, jdouble{-0.0}}}) {
      jobject object = env->CallStaticObjectMethod(java_class, wide_id, values.first, values.second);
      if (object == nullptr || env->ExceptionCheck()) return false;
      {
        art::ScopedThreadStateChange suspended(self, art::ThreadState::kNative);
        art::Runtime::Current()->GetHeap()->CollectGarbage(false);
      }
      jdouble floating = env->GetDoubleField(object, floating_field);
      if (env->ExceptionCheck() || env->GetLongField(object, wide_field) != values.first ||
          std::memcmp(&floating, &values.second, sizeof(floating)) != 0 ||
          !jit->GetCodeCache()->ContainsPc(wide_constructor->GetEntryPointFromQuickCompiledCode()) ||
          !jit->GetCodeCache()->ContainsPc(wide_factory->GetEntryPointFromQuickCompiledCode())) return false;
      env->DeleteLocalRef(object);
    }
    std::cerr << "ART JIT allocation: compiled constructor long/double stores and GC PASS\n";
  }
  {
    const char* signature = "(Ljava/lang/Object;I)Ljava/lang/Object;";
    jmethodID id = env->GetStaticMethodID(java_class, "jitNewFinalReference", signature);
    auto* factory = klass->FindClassMethod("jitNewFinalReference", signature, art::kRuntimePointerSize);
    if (id == nullptr || factory == nullptr || env->ExceptionCheck()) return false;
    jobject warm = env->CallStaticObjectMethod(java_class, id, java_class, 37);
    if (warm == nullptr || env->ExceptionCheck()) return false;
    jclass holder_class = env->GetObjectClass(warm);
    if (holder_class == nullptr || env->ExceptionCheck()) return false;
    jfieldID payload_field = env->GetFieldID(holder_class, "payload", "Ljava/lang/Object;");
    auto* constructor = self->DecodeJObject(holder_class)->AsClass()->FindClassMethod(
        "<init>", "(Ljava/lang/Object;I)V", art::kRuntimePointerSize);
    if (constructor == nullptr || payload_field == nullptr || env->ExceptionCheck()) return false;
    uint32_t literal_forms = 0;
    bool has_backedge = false, has_condition = false;
    art::CodeItemDataAccessor constructor_code(*constructor->GetDexFile(), constructor->GetCodeItem());
    for (const auto pair : constructor_code) {
      const auto opcode = pair.Inst().Opcode();
      if (pair.Inst().IsBranch()) {
        has_backedge |= pair.Inst().GetTargetOffset() < 0;
        has_condition |= !pair.Inst().IsUnconditional();
      }
      if (opcode >= art::Instruction::CONST_4 && opcode <= art::Instruction::CONST_WIDE_HIGH16)
        literal_forms |= 1u << (opcode - art::Instruction::CONST_4);
    }
    if (literal_forms != 0xffu || !has_backedge || !has_condition) {
      std::cerr << "ART JIT constructor literal opcode coverage incomplete: " << literal_forms << "\n";
      return false;
    }
    std::cerr << "ART JIT constructor: all 8 DEX numeric literals, conditional and backedge present\n";
    env->DeleteLocalRef(warm);
    // ARM64 publishes initialized classes in batches. Complete the ordinary
    // AOSP visibility callback before explicitly requesting compilation.
    {
      art::ScopedThreadStateChange suspended(self, art::ThreadState::kNative);
      art::Runtime::Current()->GetClassLinker()->MakeInitializedClassesVisiblyInitialized(self, true);
    }
    for (auto* target : {constructor, factory}) {
      if (!jit->CompileMethod(target, self, art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(target->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT final reference compile failed: " << target->PrettyMethod() << "\n";
        return false;
      }
    }
    jintArray payload = env->NewIntArray(2);
    const jint markers[] = {0x12345678, -987654321};
    if (payload == nullptr || env->ExceptionCheck()) return false;
    env->SetIntArrayRegion(payload, 0, 2, markers);
    const uint64_t collections_before = art::Runtime::Current()->GetHeap()->GetGcCount();
    jobject holder = env->CallStaticObjectMethod(java_class, id, payload, 37);
    if (holder == nullptr || env->ExceptionCheck() ||
        art::Runtime::Current()->GetHeap()->GetGcCount() <= collections_before) return false;
    jobject actual = env->GetObjectField(holder, payload_field);
    if (env->ExceptionCheck() || !env->IsSameObject(actual, payload)) return false;
    env->DeleteLocalRef(actual);
    env->DeleteLocalRef(payload);
    // The final field is now the sole managed strong edge to this ordinary array.
    {
      art::ScopedThreadStateChange suspended(self, art::ThreadState::kNative);
      art::Runtime::Current()->GetHeap()->CollectGarbage(false);
    }
    actual = env->GetObjectField(holder, payload_field);
    if (actual == nullptr || env->ExceptionCheck()) return false;
    jfieldID computed_id = env->GetFieldID(holder_class, "computed", "I");
    if (computed_id == nullptr || env->ExceptionCheck()) return false;
    uint32_t expected_computed = 19;
    for (uint32_t i = 1; i <= 37; ++i) expected_computed = (expected_computed * 31u) ^ i;
    if (static_cast<uint32_t>(env->GetIntField(holder, computed_id)) != expected_computed) return false;
    for (const auto& field : {std::pair{"small", jint{-7}}, std::pair{"medium", jint{-1234}},
                              std::pair{"full", jint{0x12345678}}, std::pair{"high", jint{0x12340000}}}) {
      jfieldID field_id = env->GetFieldID(holder_class, field.first, "I");
      if (field_id == nullptr || env->ExceptionCheck() ||
          env->GetIntField(holder, field_id) != field.second) return false;
    }
    for (const auto& field : {std::pair{"wideSmall", jlong{-12345}},
                              std::pair{"wideMedium", jlong{-123456789}},
                              std::pair{"wideFull", jlong{0x123456789abcdef}},
                              std::pair{"wideHigh", jlong{0x1234000000000000}}}) {
      jfieldID field_id = env->GetFieldID(holder_class, field.first, "J");
      if (field_id == nullptr || env->ExceptionCheck() ||
          env->GetLongField(holder, field_id) != field.second) return false;
    }
    jfieldID float_id = env->GetFieldID(holder_class, "floatZero", "F");
    jfieldID double_id = env->GetFieldID(holder_class, "doubleZero", "D");
    jfieldID null_id = env->GetFieldID(holder_class, "nullValue", "Ljava/lang/Object;");
    if (float_id == nullptr || double_id == nullptr || null_id == nullptr || env->ExceptionCheck())
      return false;
    const jfloat actual_float = env->GetFloatField(holder, float_id), expected_float = -0.0f;
    const jdouble actual_double = env->GetDoubleField(holder, double_id), expected_double = -0.0;
    if (env->ExceptionCheck() || std::memcmp(&actual_float, &expected_float, sizeof(jfloat)) != 0 ||
        std::memcmp(&actual_double, &expected_double, sizeof(jdouble)) != 0 ||
        env->GetObjectField(holder, null_id) != nullptr) return false;
    std::cerr << "ART JIT constructor: narrow/wide/high literals, negative float/double zero, null PASS\n";
    jint recovered[2] = {};
    env->GetIntArrayRegion(static_cast<jintArray>(actual), 0, 2, recovered);
    if (env->ExceptionCheck() || std::memcmp(recovered, markers, sizeof(markers)) != 0) return false;
    env->DeleteLocalRef(actual);
    env->DeleteLocalRef(holder);
    jobject null_holder = env->CallStaticObjectMethod(java_class, id, nullptr, 0);
    if (null_holder == nullptr || env->ExceptionCheck() ||
        env->GetIntField(null_holder, computed_id) != 7 ||
        env->GetObjectField(null_holder, payload_field) != nullptr ||
        !jit->GetCodeCache()->ContainsPc(constructor->GetEntryPointFromQuickCompiledCode()) ||
        !jit->GetCodeCache()->ContainsPc(factory->GetEntryPointFromQuickCompiledCode())) return false;
    env->DeleteLocalRef(null_holder);
    if (!CheckJitLoopCheckpoint(env, self, jit, java_class, id, constructor, factory,
                                payload_field, computed_id)) return false;
    env->DeleteLocalRef(holder_class);
    std::cerr << "ART JIT constructor: ordinary final reference across parent GC/sole-edge GC/null PASS\n";
    std::cerr << "ART JIT constructor: reference conditions, zero/nonzero loop, wrapping int math PASS\n";
  }
  // Keep allocations live so the managed heap, not the host process, reaches
  // its configured limit. No explicit GC occurs in the pressure interval.
  auto* heap = art::Runtime::Current()->GetHeap();
  constexpr jint pressure_length = 262144;
  constexpr size_t pressure_bytes = static_cast<size_t>(pressure_length) * sizeof(jint);
  if (heap->GetMaxMemory() > 512u * 1024u * 1024u) return false;
  const size_t attempts = heap->GetMaxMemory() / pressure_bytes + 32;
  jmethodID pressure_id = env->GetStaticMethodID(java_class, "jitNewArrayI", "(I)[I");
  auto* pressure_method = klass->FindClassMethod("jitNewArrayI", "(I)[I", art::kRuntimePointerSize);
  if (pressure_id == nullptr || pressure_method == nullptr) return false;
  const uint64_t gc_before = heap->GetGcCount();
  std::vector<jintArray> retained;
  bool saw_oom = false;
  for (size_t i = 0; i < attempts; ++i) {
    if (!jit->GetCodeCache()->ContainsPc(pressure_method->GetEntryPointFromQuickCompiledCode()))
      return false;
    jintArray array = static_cast<jintArray>(env->CallStaticObjectMethod(
        java_class, pressure_id, pressure_length));
    if (env->ExceptionCheck()) {
      if (array != nullptr || !ClearExpectedArrayException(env, "java/lang/OutOfMemoryError"))
        return false;
      saw_oom = true;
      break;
    }
    if (array == nullptr) return false;
    const jint marker = static_cast<jint>(i + 1);
    env->SetIntArrayRegion(array, 0, 1, &marker);
    env->SetIntArrayRegion(array, pressure_length - 1, 1, &marker);
    if (env->ExceptionCheck()) return false;
    retained.push_back(array);
  }
  const uint64_t gc_after = heap->GetGcCount();
  bool values_preserved = true;
  for (size_t i = 0; i < retained.size(); ++i) {
    jint first = 0, last = 0;
    env->GetIntArrayRegion(retained[i], 0, 1, &first);
    env->GetIntArrayRegion(retained[i], pressure_length - 1, 1, &last);
    values_preserved &= !env->ExceptionCheck() && first == static_cast<jint>(i + 1) && last == first;
    env->DeleteLocalRef(retained[i]);
  }
  if (!saw_oom || gc_after <= gc_before || !values_preserved) return false;
  {
    art::ScopedThreadStateChange suspended(self, art::ThreadState::kNative);
    heap->CollectGarbage(false);
  }
  jintArray recovered = static_cast<jintArray>(env->CallStaticObjectMethod(
      java_class, pressure_id, pressure_length));
  if (recovered == nullptr || env->ExceptionCheck() ||
      !CheckZeroArray(env, recovered, 'I', pressure_length) ||
      !jit->GetCodeCache()->ContainsPc(pressure_method->GetEntryPointFromQuickCompiledCode()))
    return false;
  env->DeleteLocalRef(recovered);
  std::cerr << "ART JIT allocation: pressure/OOME/live roots/recovery PASS retained="
            << retained.size() << " collections=" << gc_after - gc_before << "\n";
  return true;
}
}  // namespace aim_jni_acceptance_phase
