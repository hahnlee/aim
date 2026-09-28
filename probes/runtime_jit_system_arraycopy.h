#pragma once

#include <array>

namespace aim_jni_acceptance_phase {

inline bool CheckJitSystemArrayCopy(JNIEnv* env,
                                    art::Thread* self,
                                    art::jit::Jit* jit,
                                    art::Handle<art::mirror::Class> owner,
                                    jclass java_owner) {
  constexpr const char* object_sig = "(Ljava/lang/Object;ILjava/lang/Object;II)V";
  constexpr const char* char_sig = "([CI[CII)V";
  constexpr const char* byte_sig = "([BI[BII)V";
  constexpr const char* int_sig = "([II[III)V";
  const char* names[] = {"jitSystemArrayCopyObject", "jitSystemArrayCopyChar",
                         "jitSystemArrayCopyByte", "jitSystemArrayCopyInt"};
  const char* signatures[] = {object_sig, char_sig, byte_sig, int_sig};
  art::ArtMethod* methods[4]{};
  jmethodID ids[4]{};
  for (size_t index = 0; index < std::size(methods); ++index) {
    methods[index] = owner->FindClassMethod(names[index], signatures[index], art::kRuntimePointerSize);
    ids[index] = env->GetStaticMethodID(java_owner, names[index], signatures[index]);
    if (methods[index] == nullptr || ids[index] == nullptr || env->ExceptionCheck()) return false;
  }
  jclass object_class = env->FindClass("java/lang/Object");
  jobject first = env->AllocObject(java_owner);
  jobject second = env->AllocObject(java_owner);
  jclass bounds_class = env->FindClass("java/lang/ArrayIndexOutOfBoundsException");
  jclass store_class = env->FindClass("java/lang/ArrayStoreException");
  if (object_class == nullptr || first == nullptr || second == nullptr || bounds_class == nullptr ||
      store_class == nullptr || env->ExceptionCheck()) return false;

  bool ok = true;
  for (int phase = 0; phase < 3 && ok; ++phase) {
    if (phase != 0) {
      art::CompilationKind kind = phase == 1
          ? art::CompilationKind::kBaseline
          : art::CompilationKind::kOptimized;
      for (size_t index = 0; index < std::size(methods); ++index) {
        if (!jit->CompileMethod(methods[index], self, kind, false) ||
            !jit->GetCodeCache()->ContainsPc(methods[index]->GetEntryPointFromQuickCompiledCode())) {
          std::cerr << "ART JIT System.arraycopy compile failed phase=" << phase
                    << " method=" << names[index] << "\n";
          return false;
        }
      }
      art::Runtime::Current()->GetHeap()->CollectGarbage(false);
    }

    jobjectArray source = env->NewObjectArray(3, object_class, nullptr);
    jobjectArray destination = env->NewObjectArray(4, object_class, nullptr);
    env->SetObjectArrayElement(source, 0, first);
    env->SetObjectArrayElement(source, 1, second);
    env->CallStaticVoidMethod(java_owner, ids[0], source, 0, destination, 1, 3);
    jobject copied0 = env->GetObjectArrayElement(destination, 1);
    jobject copied1 = env->GetObjectArrayElement(destination, 2);
    ok &= env->IsSameObject(copied0, first) && env->IsSameObject(copied1, second);
    env->DeleteLocalRef(copied1);
    env->DeleteLocalRef(copied0);

    const jchar initial[] = {'a', 'b', 'c', 'd', 'e'};
    jcharArray chars = env->NewCharArray(5);
    env->SetCharArrayRegion(chars, 0, 5, initial);
    env->CallStaticVoidMethod(java_owner, ids[1], chars, 0, chars, 1, 4);
    jchar actual[5]{};
    env->GetCharArrayRegion(chars, 0, 5, actual);
    const jchar expected[] = {'a', 'a', 'b', 'c', 'd'};
    ok &= std::equal(std::begin(actual), std::end(actual), std::begin(expected));

    const jbyte initial_bytes[] = {1, 2, 3, 4, 5, 6};
    jbyteArray bytes = env->NewByteArray(6);
    env->SetByteArrayRegion(bytes, 0, 6, initial_bytes);
    env->CallStaticVoidMethod(java_owner, ids[2], bytes, 1, bytes, 2, 4);
    jbyte actual_bytes[6]{};
    env->GetByteArrayRegion(bytes, 0, 6, actual_bytes);
    const jbyte expected_bytes[] = {1, 2, 2, 3, 4, 5};
    ok &= std::equal(
        std::begin(actual_bytes), std::end(actual_bytes), std::begin(expected_bytes));

    const jint initial_ints[] = {11, 22, 33, 44, 55};
    jintArray int_source = env->NewIntArray(5);
    jintArray int_destination = env->NewIntArray(5);
    env->SetIntArrayRegion(int_source, 0, 5, initial_ints);
    env->CallStaticVoidMethod(java_owner, ids[3], int_source, 1, int_destination, 0, 3);
    jint actual_ints[5]{};
    env->GetIntArrayRegion(int_destination, 0, 5, actual_ints);
    const jint expected_ints[] = {22, 33, 44, 0, 0};
    ok &= std::equal(
        std::begin(actual_ints), std::end(actual_ints), std::begin(expected_ints));

    env->CallStaticVoidMethod(java_owner, ids[0], source, -1, destination, 0, 1);
    jthrowable bounds = env->ExceptionOccurred();
    env->ExceptionClear();
    ok &= bounds != nullptr && env->IsInstanceOf(bounds, bounds_class);
    jintArray ints = env->NewIntArray(1);
    env->CallStaticVoidMethod(java_owner, ids[0], source, 0, ints, 0, 1);
    jthrowable store = env->ExceptionOccurred();
    env->ExceptionClear();
    ok &= store != nullptr && env->IsInstanceOf(store, store_class);
    env->CallStaticVoidMethod(java_owner, ids[2], bytes, 0, bytes, 0, 7);
    jthrowable byte_bounds = env->ExceptionOccurred();
    env->ExceptionClear();
    ok &= byte_bounds != nullptr && env->IsInstanceOf(byte_bounds, bounds_class);
    ok &= !env->ExceptionCheck();

    env->DeleteLocalRef(byte_bounds);
    env->DeleteLocalRef(int_destination);
    env->DeleteLocalRef(int_source);
    env->DeleteLocalRef(bytes);
    env->DeleteLocalRef(store);
    env->DeleteLocalRef(ints);
    env->DeleteLocalRef(bounds);
    env->DeleteLocalRef(chars);
    env->DeleteLocalRef(destination);
    env->DeleteLocalRef(source);
  }
  env->DeleteLocalRef(store_class);
  env->DeleteLocalRef(bounds_class);
  env->DeleteLocalRef(second);
  env->DeleteLocalRef(first);
  env->DeleteLocalRef(object_class);
  if (!ok || env->ExceptionCheck()) return false;
  std::cerr << "ART JIT System.arraycopy: reference/char plus AOSP ordinary-call byte/int "
               "copy, overlap, bounds/type exceptions "
               "interpreter+baseline+optimized with CC GC PASS\n";
  return true;
}

}  // namespace aim_jni_acceptance_phase
