#pragma once

#include <jni.h>

#include <cstdint>

namespace art {
class ClassLinker;
class Thread;
template <class T>
class Handle;
namespace mirror {
class Class;
}  // namespace mirror
}  // namespace art

namespace aim_jni_acceptance_phase {

struct Results final {
  int32_t hello_answer = 0;
  int32_t native_round_trip = 0;
  int32_t arraycopy_result = 0;
};

// Runs the small JNI/DEX ABI acceptance matrix after the Android framework
// hierarchy is installed. Keeping this phase outside runtime_entry_probe.cc
// gives edits to the launcher/resource orchestration a narrow cache boundary.
int run(JNIEnv* env, art::Thread* self, art::ClassLinker* class_linker,
        art::Handle<art::mirror::Class> hello, jclass hello_class,
        Results* results);

}  // namespace aim_jni_acceptance_phase
