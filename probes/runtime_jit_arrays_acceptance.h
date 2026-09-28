#pragma once

namespace aim_jni_acceptance_phase {
inline jarray NewTypedArray(JNIEnv* env, char type, jclass element, jsize size) {
  switch (type) {
    case 'Z': return env->NewBooleanArray(size); case 'B': return env->NewByteArray(size);
    case 'C': return env->NewCharArray(size); case 'S': return env->NewShortArray(size);
    case 'I': return env->NewIntArray(size); case 'J': return env->NewLongArray(size);
    case 'F': return env->NewFloatArray(size); case 'D': return env->NewDoubleArray(size);
    case 'L': return env->NewObjectArray(size, element, nullptr); default: return nullptr;
  }
}

inline bool ClearExpectedArrayException(JNIEnv* env, const char* descriptor) {
  jthrowable exception = env->ExceptionOccurred();
  env->ExceptionClear();
  jclass expected = env->FindClass(descriptor);
  const bool matches = exception != nullptr && expected != nullptr &&
      env->IsInstanceOf(exception, expected);
  if (exception != nullptr) env->DeleteLocalRef(exception);
  if (expected != nullptr) env->DeleteLocalRef(expected);
  return matches && !env->ExceptionCheck();
}

inline bool CheckJitArrays(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                           art::Handle<art::mirror::Class> klass, jclass java_class) {
  jclass object_class = env->FindClass("java/lang/Object");
  if (object_class == nullptr || env->ExceptionCheck()) return false;
  for (char type : std::string_view("ZBCSIJFDL")) {
    std::string suffix(1, type);
    const std::string element = type == 'L' ? "Ljava/lang/Object;" : std::string(1, type);
    const std::string array = "[" + element;
    const std::string length_sig = "(" + array + ")I";
    const std::string get_sig = "(" + array + "I)" + element;
    const std::string set_sig = "(" + array + "I" + element + ")V";
    auto* length = klass->FindClassMethod(("jitArrayLength" + suffix).c_str(), length_sig.c_str(),
                                          art::kRuntimePointerSize);
    auto* getter = klass->FindClassMethod(("jitArrayGet" + suffix).c_str(), get_sig.c_str(),
                                          art::kRuntimePointerSize);
    auto* setter = klass->FindClassMethod(("jitArraySet" + suffix).c_str(), set_sig.c_str(),
                                          art::kRuntimePointerSize);
    if (length == nullptr || getter == nullptr || setter == nullptr) return false;
    const jsize size = type == 'I' ? 5000 : 5;
    jarray value_array = NewTypedArray(
        env, type, type == 'L' ? object_class : java_class, size);
    if (value_array == nullptr || env->ExceptionCheck()) return false;
    auto raw_array = self->DecodeJObject(value_array);
    uint32_t array_ref = art::mirror::CompressedReference<art::mirror::Object>::
        FromMirrorPtr(raw_array.Ptr()).AsVRegValue();
    art::JValue result;
    length->Invoke(self, &array_ref, 4, &result, "IL");
    if (self->IsExceptionPending() || result.GetI() != size) return false;
    const uint32_t index = type == 'I' ? 4097 : 3;
    const uint64_t bits = type == 'Z' ? 1 : type == 'F' ? 0x41280000u :
        type == 'D' ? UINT64_C(0x4025000000000000) : UINT64_C(0x87654321abcdef93);
    const uint64_t warm_bits = type == 'L' || type == 'Z' ? 0 :
        bits ^ UINT64_C(0x0101010101010101);
    uint32_t args[4] = {array_ref, index,
        static_cast<uint32_t>(warm_bits), static_cast<uint32_t>(warm_bits >> 32)};
    std::string get_shorty; get_shorty += type; get_shorty += "LI";
    std::string set_shorty("VLI"); set_shorty += type;
    // Resolve all three methods through the interpreter before compilation.
    setter->Invoke(self, args, (type == 'J' || type == 'D') ? 16 : 12, &result,
                   set_shorty.c_str());
    if (self->IsExceptionPending()) return false;
    args[2] = type == 'L' ? art::mirror::CompressedReference<art::mirror::Object>::
        FromMirrorPtr(klass.Get()).AsVRegValue() : static_cast<uint32_t>(bits);
    args[3] = static_cast<uint32_t>(bits >> 32);
    if (!jit->CompileMethod(length, self, art::CompilationKind::kOptimized, false) ||
        !jit->CompileMethod(getter, self, art::CompilationKind::kOptimized, false) ||
        !jit->CompileMethod(setter, self, art::CompilationKind::kOptimized, false))
      return false;
    setter->Invoke(self, args, (type == 'J' || type == 'D') ? 16 : 12, &result,
                   set_shorty.c_str());
    if (self->IsExceptionPending() ||
        !jit->GetCodeCache()->ContainsPc(setter->GetEntryPointFromQuickCompiledCode())) return false;
    uint32_t get_args[2] = {array_ref, index};
    getter->Invoke(self, get_args, 8, &result, get_shorty.c_str());
    if (self->IsExceptionPending()) return false;
    if (type == 'L') { if (result.GetL() != klass.Get()) return false; }
    else if (type == 'J' || type == 'D') { if (static_cast<uint64_t>(result.GetJ()) != bits) return false; }
    else {
      uint32_t expected = type == 'Z' ? 1 :
          type == 'B' ? static_cast<uint32_t>(static_cast<int32_t>(static_cast<int8_t>(bits))) :
          type == 'S' ? static_cast<uint32_t>(static_cast<int32_t>(static_cast<int16_t>(bits))) :
          type == 'C' ? static_cast<uint16_t>(bits) : static_cast<uint32_t>(bits);
      if (static_cast<uint32_t>(result.GetI()) != expected) return false;
    }
    if (type == 'L') {
      jobject hello_value = env->AllocObject(java_class);
      jobjectArray covariant = env->NewObjectArray(3, java_class, nullptr);
      if (hello_value == nullptr || covariant == nullptr || env->ExceptionCheck()) return false;
      const uint32_t covariant_ref = art::mirror::CompressedReference<art::mirror::Object>::
          FromMirrorPtr(self->DecodeJObject(covariant).Ptr()).AsVRegValue();
      const uint32_t hello_ref = art::mirror::CompressedReference<art::mirror::Object>::
          FromMirrorPtr(self->DecodeJObject(hello_value).Ptr()).AsVRegValue();
      uint32_t compatible_args[3] = {covariant_ref, 1, hello_ref};
      setter->Invoke(self, compatible_args, 12, &result, set_shorty.c_str());
      jobject stored = env->GetObjectArrayElement(covariant, 1);
      const bool compatible_stored = !self->IsExceptionPending() &&
          env->IsSameObject(stored, hello_value);
      if (stored != nullptr) env->DeleteLocalRef(stored);
      if (!compatible_stored) return false;
      uint32_t incompatible_args[3] = {covariant_ref, 1, args[2]};
      setter->Invoke(self, incompatible_args, 12, &result, set_shorty.c_str());
      if (!self->IsExceptionPending() ||
          !ClearExpectedArrayException(env, "java/lang/ArrayStoreException")) return false;
      stored = env->GetObjectArrayElement(covariant, 1);
      const bool incompatible_preserved = !env->ExceptionCheck() &&
          env->IsSameObject(stored, hello_value);
      if (stored != nullptr) env->DeleteLocalRef(stored);
      if (!incompatible_preserved) return false;

      jclass char_sequence = env->FindClass("java/lang/CharSequence");
      jstring string_value = env->NewStringUTF("assignable");
      jobjectArray interface_array = char_sequence == nullptr ? nullptr :
          env->NewObjectArray(2, char_sequence, nullptr);
      if (interface_array == nullptr || string_value == nullptr || env->ExceptionCheck()) return false;
      const uint32_t interface_ref = art::mirror::CompressedReference<art::mirror::Object>::
          FromMirrorPtr(self->DecodeJObject(interface_array).Ptr()).AsVRegValue();
      const uint32_t string_ref = art::mirror::CompressedReference<art::mirror::Object>::
          FromMirrorPtr(self->DecodeJObject(string_value).Ptr()).AsVRegValue();
      uint32_t assignable_args[3] = {interface_ref, 1, string_ref};
      setter->Invoke(self, assignable_args, 12, &result, set_shorty.c_str());
      stored = env->GetObjectArrayElement(interface_array, 1);
      const bool slow_assignable_stored = !self->IsExceptionPending() &&
          env->IsSameObject(stored, string_value);
      if (stored != nullptr) env->DeleteLocalRef(stored);
      if (!slow_assignable_stored) return false;
      env->DeleteLocalRef(interface_array);
      env->DeleteLocalRef(string_value);
      env->DeleteLocalRef(char_sequence);

      uint32_t null_args[3] = {covariant_ref, 1, 0};
      setter->Invoke(self, null_args, 12, &result, set_shorty.c_str());
      if (self->IsExceptionPending() || env->GetObjectArrayElement(covariant, 1) != nullptr)
        return false;
      env->DeleteLocalRef(covariant);
      env->DeleteLocalRef(hello_value);
    }
    if (type == 'I') {
      uint32_t null_get_args[2] = {0, 0};
      getter->Invoke(self, null_get_args, 8, &result, get_shorty.c_str());
      if (!self->IsExceptionPending() ||
          !ClearExpectedArrayException(env, "java/lang/NullPointerException")) return false;
      uint32_t bounds_get_args[2] = {array_ref, static_cast<uint32_t>(size)};
      getter->Invoke(self, bounds_get_args, 8, &result, get_shorty.c_str());
      if (!self->IsExceptionPending() ||
          !ClearExpectedArrayException(env, "java/lang/ArrayIndexOutOfBoundsException")) return false;
    }
    {
      art::ScopedThreadStateChange suspended(self, art::ThreadState::kNative);
      art::Runtime::Current()->GetHeap()->CollectGarbage(false);
    }
    array_ref = art::mirror::CompressedReference<art::mirror::Object>::
        FromMirrorPtr(self->DecodeJObject(value_array).Ptr()).AsVRegValue();
    length->Invoke(self, &array_ref, 4, &result, "IL");
    if (self->IsExceptionPending() || result.GetI() != size ||
        !jit->GetCodeCache()->ContainsPc(length->GetEntryPointFromQuickCompiledCode()) ||
        !jit->GetCodeCache()->ContainsPc(getter->GetEntryPointFromQuickCompiledCode())) return false;
    env->DeleteLocalRef(value_array);
  }
  env->DeleteLocalRef(object_class);
  return true;
}
}  // namespace aim_jni_acceptance_phase
