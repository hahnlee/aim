#pragma once

#include <array>

namespace aim_jni_acceptance_phase {

inline uint32_t ReferenceCrc32(const uint8_t* bytes, size_t size) {
  uint32_t crc = UINT32_MAX;
  for (size_t index = 0; index < size; ++index) {
    crc ^= bytes[index];
    for (int bit = 0; bit < 8; ++bit) {
      crc = (crc >> 1) ^ (UINT32_C(0xedb88320) & (0u - (crc & 1u)));
    }
  }
  return ~crc;
}

inline bool CheckJitCrc32(JNIEnv* env,
                          art::Thread* self,
                          art::jit::Jit* jit,
                          art::Handle<art::mirror::Class> owner,
                          jclass java_owner) {
  jclass crc_class = env->FindClass("java/util/zip/CRC32");
  jclass byte_buffer_class = env->FindClass("java/nio/ByteBuffer");
  if (crc_class == nullptr || byte_buffer_class == nullptr || env->ExceptionCheck()) return false;
  art::ObjPtr<art::mirror::Class> crc_mirror = self->DecodeJObject(crc_class)->AsClass();
  struct MethodSpec { const char* signature; };
  constexpr MethodSpec specs[] = {
      {"(I)V"}, {"([BII)V"}, {"(Ljava/nio/ByteBuffer;)V"},
  };
  std::array<art::ArtMethod*, std::size(specs)> methods{};
  std::array<jmethodID, std::size(specs)> ids{};
  for (size_t index = 0; index < std::size(specs); ++index) {
    methods[index] = crc_mirror->FindClassMethod(
        "update", specs[index].signature, art::kRuntimePointerSize);
    ids[index] = env->GetMethodID(crc_class, "update", specs[index].signature);
    if (methods[index] == nullptr || ids[index] == nullptr || env->ExceptionCheck()) return false;
  }
  jmethodID constructor = env->GetMethodID(crc_class, "<init>", "()V");
  jmethodID get_value = env->GetMethodID(crc_class, "getValue", "()J");
  jmethodID allocate_direct =
      env->GetStaticMethodID(byte_buffer_class, "allocateDirect", "(I)Ljava/nio/ByteBuffer;");
  jmethodID put = env->GetMethodID(
      byte_buffer_class, "put", "([B)Ljava/nio/ByteBuffer;");
  jmethodID flip = env->GetMethodID(
      byte_buffer_class, "flip", "()Ljava/nio/Buffer;");
  jmethodID position = env->GetMethodID(byte_buffer_class, "position", "()I");
  if (constructor == nullptr || get_value == nullptr || allocate_direct == nullptr || put == nullptr ||
      flip == nullptr || position == nullptr || env->ExceptionCheck()) return false;
  constexpr const char* int_wrapper_signature = "(I)J";
  auto* int_wrapper = owner->FindClassMethod(
      "jitCrc32UpdateInt", int_wrapper_signature, art::kRuntimePointerSize);
  jmethodID int_wrapper_id =
      env->GetStaticMethodID(java_owner, "jitCrc32UpdateInt", int_wrapper_signature);
  if (int_wrapper == nullptr || int_wrapper_id == nullptr || env->ExceptionCheck()) return false;

  constexpr std::array<uint8_t, 19> payload{{
      0x00, 0x01, 0x7f, 0x80, 0xff, 'D', 'a', 'r', 'w', 'i', 'n', '-',
      'A', 'R', 'T', '-', 'C', 'R', 'C',
  }};
  jbyteArray bytes = env->NewByteArray(payload.size());
  env->SetByteArrayRegion(
      bytes, 0, payload.size(), reinterpret_cast<const jbyte*>(payload.data()));
  if (bytes == nullptr || env->ExceptionCheck()) return false;

  auto new_crc = [&]() { return env->NewObject(crc_class, constructor); };
  auto value = [&](jobject crc) {
    return static_cast<uint32_t>(env->CallLongMethod(crc, get_value));
  };
  bool ok = true;
  for (int phase = 0; phase < 3 && ok; ++phase) {
    if (phase != 0) {
      art::CompilationKind kind = phase == 1
          ? art::CompilationKind::kBaseline
          : art::CompilationKind::kOptimized;
      for (size_t index = 0; index < methods.size(); ++index) {
        if (!jit->CompileMethod(methods[index], self, kind, false) ||
            !jit->GetCodeCache()->ContainsPc(methods[index]->GetEntryPointFromQuickCompiledCode())) {
          std::cerr << "ART JIT CRC32 compile failed phase=" << phase
                    << " signature=" << specs[index].signature << "\n";
          return false;
        }
      }
      if (!jit->CompileMethod(int_wrapper, self, kind, false) ||
          !jit->GetCodeCache()->ContainsPc(int_wrapper->GetEntryPointFromQuickCompiledCode())) {
        return false;
      }
    }

    jvalue single_args[1]{};
    single_args[0].i = 0x80;
    uint64_t single_packed = static_cast<uint64_t>(
        env->CallStaticLongMethodA(java_owner, int_wrapper_id, single_args));
    uint32_t single_input = static_cast<uint32_t>(single_packed >> 32);
    uint32_t single_actual = static_cast<uint32_t>(single_packed);
    const uint8_t single_byte = 0x80;
    uint32_t single_expected = ReferenceCrc32(&single_byte, 1);
    ok &= single_input == 0x80;
    ok &= single_actual == single_expected;

    jobject array = new_crc();
    jvalue array_args[3]{};
    array_args[0].l = bytes;
    array_args[1].i = 3;
    array_args[2].i = 13;
    env->CallVoidMethodA(array, ids[1], array_args);
    uint32_t array_actual = value(array);
    uint32_t array_expected = ReferenceCrc32(payload.data() + 3, 13);
    ok &= array_actual == array_expected;

    jobject buffer_crc = new_crc();
    jobject buffer = env->CallStaticObjectMethod(
        byte_buffer_class, allocate_direct, static_cast<jint>(payload.size()));
    jobject put_result = env->CallObjectMethod(buffer, put, bytes);
    jobject flip_result = env->CallObjectMethod(buffer, flip);
    jvalue buffer_args[1]{};
    buffer_args[0].l = buffer;
    env->CallVoidMethodA(buffer_crc, ids[2], buffer_args);
    uint32_t buffer_actual = value(buffer_crc);
    uint32_t buffer_expected = ReferenceCrc32(payload.data(), payload.size());
    jint buffer_position = env->CallIntMethod(buffer, position);
    ok &= buffer_actual == buffer_expected;
    ok &= buffer_position == static_cast<jint>(payload.size());
    ok &= !env->ExceptionCheck();
    if (!ok) {
      std::cerr << "ART JIT CRC32 mismatch phase=" << phase
                << " single_input=" << single_input
                << " single=" << single_actual << "/" << single_expected
                << " array=" << array_actual << "/" << array_expected
                << " buffer=" << buffer_actual << "/" << buffer_expected
                << " position=" << buffer_position << "/" << payload.size()
                << " exception=" << env->ExceptionCheck() << "\n";
      env->ExceptionDescribe();
    }

    env->DeleteLocalRef(flip_result);
    env->DeleteLocalRef(put_result);
    env->DeleteLocalRef(buffer);
    env->DeleteLocalRef(buffer_crc);
    env->DeleteLocalRef(array);
  }

  if (!ok) return false;
  jobject failure = new_crc();
  env->CallVoidMethod(failure, ids[1], bytes, -1, 1);
  ok &= ClearExpectedArrayException(env, "java/lang/ArrayIndexOutOfBoundsException");
  env->CallVoidMethod(failure, ids[1], nullptr, 0, 1);
  ok &= ClearExpectedArrayException(env, "java/lang/NullPointerException");
  env->DeleteLocalRef(failure);
  env->DeleteLocalRef(bytes);
  env->DeleteLocalRef(byte_buffer_class);
  env->DeleteLocalRef(crc_class);
  if (!ok || env->ExceptionCheck()) return false;
  std::cerr << "ART JIT CRC32: update byte/array/direct-ByteBuffer interpreter+baseline+optimized "
               "known vectors, position and exceptions PASS\n";
  return true;
}

}  // namespace aim_jni_acceptance_phase
