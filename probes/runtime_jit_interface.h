#pragma once
#include "imtable-inl.h"
#include "oat/oat_quick_method_header.h"
namespace aim_jni_acceptance_phase {
inline bool CheckJitInterface(JNIEnv* env, art::Thread* self, art::jit::Jit* jit,
                              art::Handle<art::mirror::Class> owner, jclass java_owner) {
  art::jit::ScopedJitSuspend workers;
  jobject objects[3]{};
  jclass classes[2]{};
  const char* factories[] = {"jitVirtualBaseClass", "jitVirtualChildClass"};
  for (int i = 0; i < 2; ++i) {
    auto id = env->GetStaticMethodID(java_owner, factories[i], "()Ljava/lang/Class;");
    if (!id || env->ExceptionCheck()) return false;
    classes[i] = static_cast<jclass>(env->CallStaticObjectMethod(java_owner, id));
    if (!classes[i] || env->ExceptionCheck()) return false;
    objects[i + 1] = env->AllocObject(classes[i]);
    if (!objects[i + 1] || env->ExceptionCheck()) return false;
  }
  auto number = env->GetFieldID(classes[0], "number", "I");
  auto object = env->GetFieldID(classes[0], "object", "Ljava/lang/Object;");
  auto alternative = env->GetFieldID(classes[1], "alternative", "I");
  auto alternative_object = env->GetFieldID(classes[1], "alternativeObject", "Ljava/lang/Object;");
  jclass npe = env->FindClass("java/lang/NullPointerException");
  if (!number || !object || !alternative || !alternative_object || !npe || env->ExceptionCheck()) return false;
  env->SetIntField(objects[1], number, 19);
  env->SetIntField(objects[2], number, -1);
  env->SetIntField(objects[2], alternative, 73);
  env->SetObjectField(objects[1], object, java_owner);
  env->SetObjectField(objects[2], alternative_object, objects[1]);
  const char* names[] = {"jitInterfaceValue", "jitInterfaceReference", "jitInterfaceDefault", "jitInterfaceAa", "jitInterfaceBB"};
  const char* sigs[] = {"(Ldev/aim/probe/JitCallable;)I", "(Ldev/aim/probe/JitCallable;)Ljava/lang/Object;", "(Ldev/aim/probe/JitCallable;)I", "(Ldev/aim/probe/JitCallable;)I", "(Ldev/aim/probe/JitCallable;)I"};
  art::ArtMethod* methods[5]{};
  jmethodID ids[5]{};
  for (int i = 0; i < 5; ++i) {
    methods[i] = owner->FindClassMethod(names[i], sigs[i], art::kRuntimePointerSize);
    ids[i] = env->GetStaticMethodID(java_owner, names[i], sigs[i]);
    if (!methods[i] || !ids[i] || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*methods[i]->GetDexFile(), methods[i]->GetCodeItem());
    if (art::Instruction::At(code.Insns())->Opcode() != art::Instruction::INVOKE_INTERFACE) return false;
  }
  const char* range_sig = "(Ldev/aim/probe/JitCallable;IIIIIIIJDLjava/lang/Object;)J";
  const char* range_names[] = {"jitInterfaceRange", "jitInterfaceRangeAa", "jitInterfaceRangeBB"};
  art::ArtMethod* ranges[3]{};
  jmethodID range_ids[3]{};
  for (int i = 0; i < 3; ++i) {
    ranges[i] = owner->FindClassMethod(range_names[i], range_sig, art::kRuntimePointerSize);
    range_ids[i] = env->GetStaticMethodID(java_owner, range_names[i], range_sig);
    if (!ranges[i] || !range_ids[i] || env->ExceptionCheck()) return false;
    art::CodeItemDataAccessor code(*ranges[i]->GetDexFile(), ranges[i]->GetCodeItem());
    if (art::Instruction::At(code.Insns())->Opcode() != art::Instruction::INVOKE_INTERFACE_RANGE) return false;
  }
  unsigned cases = 0, range_cases = 0;
  for (int phase = 0; phase < 3; ++phase) {
    if (phase) {
      art::CodeItemDataAccessor code(*methods[2]->GetDexFile(), methods[2]->GetCodeItem());
      auto* target = methods[2]->GetDexCache()->GetResolvedMethod(art::Instruction::At(code.Insns())->VRegB_35c());
      if (!target || !target->IsDefault() || !target->GetDeclaringClass()->IsInterface() || !target->GetCodeItem()) return false;
      auto resolve = [&](art::ArtMethod* caller) {
        art::CodeItemDataAccessor item(*caller->GetDexFile(), caller->GetCodeItem());
        const auto* instruction = art::Instruction::At(item.Insns());
        uint32_t index = instruction->Opcode() == art::Instruction::INVOKE_INTERFACE_RANGE ? instruction->VRegB_3rc() : instruction->VRegB_35c();
        auto* linker = art::Runtime::Current()->GetClassLinker();
        auto type = linker->LookupResolvedType(caller->GetDexFile()->GetMethodId(index).class_idx_, caller);
        return type != nullptr ? linker->FindResolvedMethod(type, caller->GetDexCache(), caller->GetClassLoader(), index) : nullptr;
      };
      auto* aa = resolve(methods[3]);
      auto* bb = resolve(methods[4]);
      if (!aa || !bb || aa == bb || aa->GetImtIndex() != bb->GetImtIndex()) return false;
      auto* base = static_cast<art::mirror::Class*>(self->DecodeJObject(classes[0]).Ptr());
      auto* entry = base->GetImt(art::kRuntimePointerSize)->Get(aa->GetImtIndex(), art::kRuntimePointerSize);
      if (!entry || !entry->IsRuntimeMethod() || !entry->GetImtConflictTable(art::kRuntimePointerSize)) return false;
      std::cerr << "ART JIT interface collision: distinct Aa/BB share runtime conflict entry slot=" << aa->GetImtIndex() << "\n";
      aa = resolve(ranges[1]);
      bb = resolve(ranges[2]);
      if (!aa || !bb || aa == bb || aa->GetImtIndex() != bb->GetImtIndex()) return false;
      entry = base->GetImt(art::kRuntimePointerSize)->Get(aa->GetImtIndex(), art::kRuntimePointerSize);
      if (!entry || !entry->IsRuntimeMethod() || !entry->GetImtConflictTable(art::kRuntimePointerSize)) return false;
      std::cerr << "ART JIT interface range collision: slot=" << aa->GetImtIndex() << "\n";
    }
    if (phase) for (auto* method : methods) {
      if (!jit->CompileMethod(method, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(method->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT interface compile failed: " << method->PrettyMethod() << "\n";
        return false;
      }
    }
    if (phase) for (auto* range : ranges) {
      if (!jit->CompileMethod(range, self, phase == 1 ? art::CompilationKind::kBaseline : art::CompilationKind::kOptimized, false) ||
          !jit->GetCodeCache()->ContainsPc(range->GetEntryPointFromQuickCompiledCode())) return false;
    }
    if (phase == 1) for (auto* method : {methods[3], methods[4], ranges[1], ranges[2]}) {
      const auto* header = art::OatQuickMethodHeader::FromEntryPoint(method->GetEntryPointFromQuickCompiledCode());
      uint32_t offset = art::mirror::Class::ImtPtrOffset(art::kRuntimePointerSize).Uint32Value();
      if (offset % 8 != 0 || offset / 8 >= 4096) return false;
      uint32_t load_imt = 0xf9400000u | ((offset / 8) << 10);  // ldr x0, [x0, #imt]
      bool present = false;
      for (size_t pc = 0; pc + sizeof(uint32_t) <= header->GetCodeSize(); pc += sizeof(uint32_t)) {
        uint32_t instruction;
        std::memcpy(&instruction, header->GetCode() + pc, sizeof(instruction));
        present |= instruction == load_imt;
      }
      if (!present) return false;
    }
    for (int i = 0; i < 3; ++i) for (int kind = 0; kind < 5; ++kind) {
      jobject reference = nullptr;
      jint value = 0;
      if (kind != 1) value = env->CallStaticIntMethod(java_owner, ids[kind], objects[i]);
      else reference = env->CallStaticObjectMethod(java_owner, ids[kind], objects[i]);
      jthrowable exception = env->ExceptionOccurred();
      if (exception) env->ExceptionClear();
      bool correct = i == 0 ? exception && env->IsInstanceOf(exception, npe) : !exception;
      if (correct && i > 0) correct = kind != 1 ? value == (kind == 3 ? 401 : kind == 4 ? 809 : (i == 1 ? 19 : 73) + (kind == 2 ? 101 : 0)) :
          env->IsSameObject(reference, i == 1 ? jobject(java_owner) : objects[1]);
      if (reference) env->DeleteLocalRef(reference);
      if (exception) env->DeleteLocalRef(exception);
      if (!correct) return false;
      ++cases;
    }
    for (int variant = 0; variant < 3; ++variant) for (int receiver = 0; receiver < 3; ++receiver)
      for (jlong wide : {jlong(0x123456789abcdefLL), jlong(-0x123456789abcdefLL)})
        for (jdouble real : {jdouble(-1234.5), jdouble(0)})
          for (jobject payload : {jobject(nullptr), jobject(java_owner)}) for (jint last : {77, 78}) {
            jvalue args[11]{};
            args[0].l = objects[receiver];
            for (int i = 1; i <= 7; ++i) args[i].i = i * 11;
            args[7].i = last;
            args[8].j = wide;
            args[9].d = real;
            args[10].l = payload;
            uint64_t before = art::Runtime::Current()->GetHeap()->GetGcCount();
            jlong actual = env->CallStaticLongMethodA(java_owner, range_ids[variant], args);
            jthrowable exception = env->ExceptionOccurred();
            if (exception) env->ExceptionClear();
            jlong expected = last == 77 && real == -1234.5 && payload ? wide : -1;
            if (variant == 2 || (variant == 0 && receiver == 2)) expected = -expected;
            bool correct = receiver == 0 ? exception && env->IsInstanceOf(exception, npe) :
                !exception && actual == expected && art::Runtime::Current()->GetHeap()->GetGcCount() > before;
            if (exception) env->DeleteLocalRef(exception);
            if (!correct) {
              std::cerr << "ART JIT interface range failed phase=" << phase << " receiver=" << receiver << "\n";
              return false;
            }
            ++range_cases;
          }
  }
  for (auto object : objects) if (object) env->DeleteLocalRef(object);
  for (auto klass : classes) env->DeleteLocalRef(klass);
  env->DeleteLocalRef(npe);
  std::cerr << "ART JIT interface: DEX invoke-interface/default/override/reference/null PASS cases=" << cases << "\n";
  std::cerr << "ART JIT interface range: stack/wide/FP/reference/override/callee-GC/null PASS cases=" << range_cases << "\n";
  return !env->ExceptionCheck();
}
}  // namespace aim_jni_acceptance_phase
