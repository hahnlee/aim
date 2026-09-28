#pragma once

#include <array>

namespace aim_jni_acceptance_phase {

inline bool CheckJitStringIntrinsics(JNIEnv* env,
                                     art::Thread* self,
                                     art::jit::Jit* jit,
                                     art::Handle<art::mirror::Class> owner,
                                     jclass java_owner) {
  constexpr const char* relations_signature =
      "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/Object;II)J";
  constexpr const char* builder_signature =
      "(Ljava/lang/Object;Ljava/lang/String;Ljava/lang/CharSequence;[CZCIJFD)Ljava/lang/String;";
  constexpr const char* buffer_signature = "(Ljava/lang/String;)Ljava/lang/String;";
  struct MethodSpec {
    const char* name;
    const char* signature;
  };
  constexpr MethodSpec specs[] = {
      {"jitStringRelations", relations_signature},
      {"jitStringBuilder", builder_signature},
      {"jitStringBuffer", buffer_signature},
  };
  std::array<art::ArtMethod*, std::size(specs)> methods{};
  std::array<jmethodID, std::size(specs)> ids{};
  for (size_t index = 0; index < std::size(specs); ++index) {
    methods[index] = owner->FindClassMethod(
        specs[index].name, specs[index].signature, art::kRuntimePointerSize);
    ids[index] = env->GetStaticMethodID(java_owner, specs[index].name, specs[index].signature);
    if (methods[index] == nullptr || ids[index] == nullptr || env->ExceptionCheck()) return false;
  }

  jstring value = env->NewStringUTF("Darwin Android ART Darwin");
  jstring needle = env->NewStringUTF("Darwin");
  jstring sequence = env->NewStringUTF(" sequence ");
  jcharArray chars = env->NewCharArray(3);
  const jchar char_values[] = {'A', 'R', 'T'};
  env->SetCharArrayRegion(chars, 0, 3, char_values);
  if (value == nullptr || needle == nullptr || sequence == nullptr || chars == nullptr ||
      env->ExceptionCheck()) return false;

  auto relations = [&]() {
    jvalue args[5]{};
    args[0].l = value;
    args[1].l = needle;
    args[2].l = value;
    args[3].i = 'D';
    args[4].i = 2;
    return env->CallStaticLongMethodA(java_owner, ids[0], args);
  };
  auto builder = [&]() {
    jvalue args[10]{};
    args[0].l = value;
    args[1].l = needle;
    args[2].l = sequence;
    args[3].l = chars;
    args[4].z = JNI_TRUE;
    args[5].c = '!';
    args[6].i = -1234567;
    args[7].j = INT64_C(0x123456789abcdef);
    args[8].f = -3.25f;
    args[9].d = 7.125;
    return static_cast<jstring>(env->CallStaticObjectMethodA(java_owner, ids[1], args));
  };
  auto buffer = [&]() {
    return static_cast<jstring>(env->CallStaticObjectMethod(java_owner, ids[2], value));
  };
  jlong expected_relations = relations();
  jstring expected_builder = builder();
  jstring expected_buffer = buffer();
  jclass string_class = env->FindClass("java/lang/String");
  jmethodID equals = string_class == nullptr
      ? nullptr
      : env->GetMethodID(string_class, "equals", "(Ljava/lang/Object;)Z");
  if (expected_builder == nullptr || expected_buffer == nullptr || equals == nullptr ||
      env->ExceptionCheck()) return false;

  for (int phase = 1; phase < 3; ++phase) {
    art::CompilationKind kind = phase == 1
        ? art::CompilationKind::kBaseline
        : art::CompilationKind::kOptimized;
    for (size_t index = 0; index < methods.size(); ++index) {
      if (!jit->CompileMethod(methods[index], self, kind, false) ||
          !jit->GetCodeCache()->ContainsPc(methods[index]->GetEntryPointFromQuickCompiledCode())) {
        std::cerr << "ART JIT String intrinsic compile failed phase=" << phase
                  << " method=" << specs[index].name << "\n";
        return false;
      }
    }
    art::Runtime::Current()->GetHeap()->CollectGarbage(false);
    jstring actual_builder = builder();
    jstring actual_buffer = buffer();
    bool matches = relations() == expected_relations &&
        actual_builder != nullptr && actual_buffer != nullptr &&
        env->CallBooleanMethod(expected_builder, equals, actual_builder) == JNI_TRUE &&
        env->CallBooleanMethod(expected_buffer, equals, actual_buffer) == JNI_TRUE;
    env->DeleteLocalRef(actual_builder);
    env->DeleteLocalRef(actual_buffer);
    if (!matches || env->ExceptionCheck()) return false;
  }

  jvalue null_receiver_args[5]{};
  null_receiver_args[1].l = needle;
  env->CallStaticLongMethodA(java_owner, ids[0], null_receiver_args);
  bool null_receiver = ClearExpectedArrayException(env, "java/lang/NullPointerException");
  jvalue null_needle_args[5]{};
  null_needle_args[0].l = value;
  null_needle_args[2].l = value;
  env->CallStaticLongMethodA(java_owner, ids[0], null_needle_args);
  bool null_needle = ClearExpectedArrayException(env, "java/lang/NullPointerException");

  env->DeleteLocalRef(string_class);
  env->DeleteLocalRef(expected_buffer);
  env->DeleteLocalRef(expected_builder);
  env->DeleteLocalRef(chars);
  env->DeleteLocalRef(sequence);
  env->DeleteLocalRef(needle);
  env->DeleteLocalRef(value);
  if (!null_receiver || !null_needle || env->ExceptionCheck()) return false;
  std::cerr << "ART JIT String intrinsics: compare/equals/indexOf + StringBuilder/StringBuffer "
               "interpreter+baseline+optimized with CC GC/exceptions PASS\n";
  return true;
}

}  // namespace aim_jni_acceptance_phase
