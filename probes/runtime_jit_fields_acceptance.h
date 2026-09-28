#pragma once

namespace aim_jni_acceptance_phase {
inline bool CheckJitFields(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                           art::Handle<art::mirror::Class> klass, jclass java_class) {
  jobject holder = env->AllocObject(java_class);
  if (holder == nullptr || env->ExceptionCheck()) return false;
  art::StackHandleScope<1> roots(self);
  auto receiver = roots.NewHandle(self->DecodeJObject(holder));
  bool ok = true;
  for (char mode : {'I', 'V', 'S', 'T'}) {
    const bool is_static = mode == 'S' || mode == 'T';
    for (char type : std::string_view("ZBCSIJFDL")) {
      std::string suffix; suffix += mode; suffix += type;
      const std::string descriptor = type == 'L' ? "Ljava/lang/Object;" : std::string(1, type);
      const std::string receiver_sig = is_static ? "" : "Ldev/aim/probe/Hello;";
      const std::string get_sig = "(" + receiver_sig + ")" + descriptor;
      const std::string set_sig = "(" + receiver_sig + descriptor + ")V";
      auto* getter = klass->FindClassMethod(("jitGet" + suffix).c_str(), get_sig.c_str(), art::kRuntimePointerSize);
      auto* setter = klass->FindClassMethod(("jitSet" + suffix).c_str(), set_sig.c_str(), art::kRuntimePointerSize);
      if (getter == nullptr || setter == nullptr) { ok = false; break; }
      std::string get_shorty(1, type), set_shorty("V");
      if (!is_static) { get_shorty += 'L'; set_shorty += 'L'; }
      set_shorty += type;
      const uint64_t bits = type == 'Z' ? 1 : type == 'F' ? 0x41280000u :
          type == 'D' ? UINT64_C(0x4025000000000000) : UINT64_C(0x87654321abcdef93);
      for (int phase = 0; phase < 2; ++phase) {
        // Warm resolution through the interpreter before compiling either accessor.
        if (phase == 1 &&
            (!jit->CompileMethod(getter, self, art::CompilationKind::kOptimized, false) ||
             !jit->CompileMethod(setter, self, art::CompilationKind::kOptimized, false))) {
          ok = false; break;
        }
        uint32_t args[3] = {};
        unsigned words = 0;
        if (!is_static) args[words++] = art::mirror::CompressedReference<art::mirror::Object>::
            FromMirrorPtr(receiver.Get()).AsVRegValue();
        args[words++] = type == 'L' ? art::mirror::CompressedReference<art::mirror::Object>::
            FromMirrorPtr(klass.Get()).AsVRegValue() : static_cast<uint32_t>(bits);
        if (type == 'J' || type == 'D') args[words++] = bits >> 32;
        art::JValue ignored;
        setter->Invoke(self, args, words * 4, &ignored, set_shorty.c_str());
        if (self->IsExceptionPending()) { ok = false; break; }
        if (phase == 1) {
          art::ScopedThreadStateChange suspended(self, art::ThreadState::kNative);
          art::Runtime::Current()->GetHeap()->CollectGarbage(false);
        }
        uint32_t object_arg = art::mirror::CompressedReference<art::mirror::Object>::
            FromMirrorPtr(receiver.Get()).AsVRegValue();
        art::JValue actual;
        getter->Invoke(self, is_static ? nullptr : &object_arg, is_static ? 0 : 4,
                       &actual, get_shorty.c_str());
        if (self->IsExceptionPending()) { ok = false; break; }
        if (type == 'L') ok &= actual.GetL() == klass.Get();
        else if (type == 'J' || type == 'D') ok &= static_cast<uint64_t>(actual.GetJ()) == bits;
        else {
          uint32_t expected = type == 'Z' ? 1 :
              type == 'B' ? static_cast<uint32_t>(static_cast<int32_t>(static_cast<int8_t>(bits))) :
              type == 'S' ? static_cast<uint32_t>(static_cast<int32_t>(static_cast<int16_t>(bits))) :
              type == 'C' ? static_cast<uint16_t>(bits) : static_cast<uint32_t>(bits);
          ok &= static_cast<uint32_t>(actual.GetI()) == expected;
        }
        if (phase == 1) ok &= jit->GetCodeCache()->ContainsPc(getter->GetEntryPointFromQuickCompiledCode()) &&
            jit->GetCodeCache()->ContainsPc(setter->GetEntryPointFromQuickCompiledCode());
      }
      if (!ok) { std::cerr << "ART JIT field matrix failed: " << suffix << '\n'; break; }
    }
    if (!ok) break;
  }
  env->DeleteLocalRef(holder);
  return ok;
}
}  // namespace aim_jni_acceptance_phase
