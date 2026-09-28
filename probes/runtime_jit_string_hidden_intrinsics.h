#pragma once

#include <array>

namespace aim_jni_acceptance_phase {

inline bool CheckJitHiddenStringIntrinsics(JNIEnv* env,
                                           art::Thread* self,
                                           art::jit::Jit* jit,
                                           art::Handle<art::mirror::Class> app_owner) {
  art::StackHandleScope<2> hs(self);
  art::Handle<art::mirror::ClassLoader> app_loader = hs.NewHandle(app_owner->GetClassLoader());
  constexpr const char* descriptor = "Ljava/lang/JitStringHidden;";
  art::ObjPtr<art::mirror::Class> mirror =
      art::Runtime::Current()->GetClassLinker()->FindClass(
          self, descriptor, std::char_traits<char>::length(descriptor), app_loader);
  if (mirror == nullptr || self->IsExceptionPending()) return false;
  art::Handle<art::mirror::Class> handle = hs.NewHandle(mirror);
  if (!art::Runtime::Current()->GetClassLinker()->EnsureInitialized(self, handle, true, true)) {
    return false;
  }
  art::Runtime::Current()->GetClassLinker()->MakeInitializedClassesVisiblyInitialized(
      self, /*wait=*/ true);
  jclass fixture = self->GetJniEnv()->AddLocalReference<jclass>(mirror);

  struct MethodSpec { const char* name; const char* signature; };
  constexpr MethodSpec specs[] = {
      {"getCharsNoCheck", "(Ljava/lang/String;II[CI)V"},
      {"newStringFromBytes", "([BIII)Ljava/lang/String;"},
      {"newStringFromChars", "(II[C)Ljava/lang/String;"},
      {"newStringFromString", "(Ljava/lang/String;)Ljava/lang/String;"},
  };
  std::array<art::ArtMethod*, std::size(specs)> methods{};
  std::array<jmethodID, std::size(specs)> ids{};
  for (size_t index = 0; index < std::size(specs); ++index) {
    methods[index] = mirror->FindClassMethod(
        specs[index].name, specs[index].signature, art::kRuntimePointerSize);
    ids[index] = env->GetStaticMethodID(fixture, specs[index].name, specs[index].signature);
    if (methods[index] == nullptr || ids[index] == nullptr || env->ExceptionCheck()) return false;
  }

  const jbyte byte_values[] = {'x', 'A', 'R', 'T', 'y'};
  jbyteArray bytes = env->NewByteArray(std::size(byte_values));
  env->SetByteArrayRegion(bytes, 0, std::size(byte_values), byte_values);
  const jchar char_values[] = {'x', 'D', 0x03a9, 'y'};
  jcharArray chars = env->NewCharArray(std::size(char_values));
  env->SetCharArrayRegion(chars, 0, std::size(char_values), char_values);
  jstring source = env->NewString(char_values + 1, 2);
  jcharArray copied = env->NewCharArray(6);
  jclass string_class = env->FindClass("java/lang/String");
  jmethodID equals = string_class == nullptr
      ? nullptr
      : env->GetMethodID(string_class, "equals", "(Ljava/lang/Object;)Z");
  if (bytes == nullptr || chars == nullptr || source == nullptr || copied == nullptr ||
      equals == nullptr || env->ExceptionCheck()) return false;

  auto from_bytes = [&]() {
    return static_cast<jstring>(
        env->CallStaticObjectMethod(fixture, ids[1], bytes, 0, 1, 3));
  };
  auto from_chars = [&]() {
    return static_cast<jstring>(env->CallStaticObjectMethod(fixture, ids[2], 1, 2, chars));
  };
  auto from_string = [&]() {
    return static_cast<jstring>(env->CallStaticObjectMethod(fixture, ids[3], source));
  };
  auto copy_chars = [&]() {
    const jchar sentinel[] = {'?', '?', '?', '?', '?', '?'};
    env->SetCharArrayRegion(copied, 0, std::size(sentinel), sentinel);
    env->CallStaticVoidMethod(fixture, ids[0], source, 0, 2, copied, 2);
  };

  jstring expected_bytes = from_bytes();
  jstring expected_chars = from_chars();
  jstring expected_string = from_string();
  copy_chars();
  jchar expected_copy[6]{};
  env->GetCharArrayRegion(copied, 0, 6, expected_copy);
  if (expected_bytes == nullptr || expected_chars == nullptr || expected_string == nullptr ||
      env->ExceptionCheck()) return false;

  bool ok = true;
  for (int phase = 1; phase < 3 && ok; ++phase) {
    art::CompilationKind kind = phase == 1
        ? art::CompilationKind::kBaseline
        : art::CompilationKind::kOptimized;
    for (size_t index = 0; index < methods.size(); ++index) {
      if (!jit->CompileMethod(methods[index], self, kind, false) ||
          !jit->GetCodeCache()->ContainsPc(methods[index]->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT hidden String compile failed phase=" << phase
                  << " method=" << specs[index].name << "\n";
        return false;
      }
    }
    art::Runtime::Current()->GetHeap()->CollectGarbage(false);
    jstring actual_bytes = from_bytes();
    jstring actual_chars = from_chars();
    jstring actual_string = from_string();
    copy_chars();
    jchar actual_copy[6]{};
    env->GetCharArrayRegion(copied, 0, 6, actual_copy);
    ok &= actual_bytes != nullptr && actual_chars != nullptr && actual_string != nullptr;
    ok &= env->CallBooleanMethod(expected_bytes, equals, actual_bytes) == JNI_TRUE;
    ok &= env->CallBooleanMethod(expected_chars, equals, actual_chars) == JNI_TRUE;
    ok &= env->CallBooleanMethod(expected_string, equals, actual_string) == JNI_TRUE;
    ok &= std::equal(std::begin(expected_copy), std::end(expected_copy), std::begin(actual_copy));
    ok &= !env->ExceptionCheck();
    env->DeleteLocalRef(actual_string);
    env->DeleteLocalRef(actual_chars);
    env->DeleteLocalRef(actual_bytes);
  }

  env->CallStaticObjectMethod(fixture, ids[1], nullptr, 0, 0, 0);
  ok &= ClearExpectedArrayException(env, "java/lang/NullPointerException");
  env->CallStaticObjectMethod(fixture, ids[3], nullptr);
  ok &= ClearExpectedArrayException(env, "java/lang/NullPointerException");

  env->DeleteLocalRef(expected_string);
  env->DeleteLocalRef(expected_chars);
  env->DeleteLocalRef(expected_bytes);
  env->DeleteLocalRef(string_class);
  env->DeleteLocalRef(copied);
  env->DeleteLocalRef(source);
  env->DeleteLocalRef(chars);
  env->DeleteLocalRef(bytes);
  env->DeleteLocalRef(fixture);
  if (!ok || env->ExceptionCheck()) return false;
  std::cerr << "ART JIT hidden String intrinsics: getCharsNoCheck + StringFactory bytes/chars/string "
               "trusted-boot DEX interpreter+baseline+optimized with CC GC/exceptions PASS\n";
  return true;
}

}  // namespace aim_jni_acceptance_phase
