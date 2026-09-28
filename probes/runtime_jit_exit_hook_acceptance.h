#pragma once

#include "gc/scoped_gc_critical_section.h"
#include "instrumentation.h"
#include "oat/oat_quick_method_header.h"
#include "oat/stack_map.h"
#include "thread_list.h"

namespace aim_jni_acceptance_phase {

class ReferenceExitListener final : public art::instrumentation::InstrumentationListener {
 public:
  ReferenceExitListener(art::ArtMethod* compiled, art::ArtMethod* native,
                        art::Handle<art::mirror::Class> expected)
      : compiled_(compiled), native_(native), expected_(expected) {}
  void MethodEntered(art::Thread*, art::ArtMethod*) override {}
  void MethodExited(art::Thread*, art::ArtMethod*, art::instrumentation::OptionalFrame,
                    art::JValue&) override {}
  void MethodExited(art::Thread* self, art::ArtMethod* method,
                    art::instrumentation::OptionalFrame frame,
                    art::MutableHandle<art::mirror::Object>& result) override {
    if (method != compiled_ && method != native_) return;
    if (inside_) { ok = false; return; }
    inside_ = true;
    const size_t slot = method == compiled_ ? 0 : 1;
    ++calls[slot];
    ok &= !frame.has_value();
    ok &= result.Get() == (expect_null ? nullptr : expected_.Get());
    if (!expect_null) {
      {
        art::ScopedThreadStateChange suspended(self, art::ThreadState::kNative);
        art::Runtime::Current()->GetHeap()->CollectGarbage(false);
      }
      ++collections[slot];
      ok &= result.Get() == expected_.Get();
    }
    inside_ = false;
  }
  void MethodUnwind(art::Thread*, art::ArtMethod*, uint32_t) override {}
  void DexPcMoved(art::Thread*, art::Handle<art::mirror::Object>, art::ArtMethod*, uint32_t) override {}
  void FieldRead(art::Thread*, art::Handle<art::mirror::Object>, art::ArtMethod*, uint32_t,
                 art::ArtField*) override {}
  void FieldWritten(art::Thread*, art::Handle<art::mirror::Object>, art::ArtMethod*, uint32_t,
                    art::ArtField*, const art::JValue&) override {}
  void ExceptionThrown(art::Thread*, art::Handle<art::mirror::Throwable>) override {}
  void ExceptionHandled(art::Thread*, art::Handle<art::mirror::Throwable>) override {}
  void Branch(art::Thread*, art::ArtMethod*, uint32_t, int32_t) override {}
  void WatchedFramePop(art::Thread*, const art::ShadowFrame&) override {}
  bool ok = true;
  bool expect_null = true;
  unsigned calls[2] = {};
  unsigned collections[2] = {};
 private:
  art::ArtMethod* compiled_;
  art::ArtMethod* native_;
  art::Handle<art::mirror::Class> expected_;
  bool inside_ = false;
};

// Must be last: tracing cleanup can invalidate all compiled methods.
inline bool CheckReferenceExitHooks(art::Thread* self, art::jit::Jit* jit,
                                    art::ArtMethod* identity, art::ArtMethod* native_identity,
                                    art::Handle<art::mirror::Class> expected) {
  art::jit::ScopedJitSuspend workers;
  if (art::Runtime::Current()->IsJavaDebuggable()) return false;
  if (jit->GetCodeCache()->ContainsPc(identity->GetEntryPointFromQuickCompiledCode())) return false;
  jit->GetJitCompiler()->SetDebuggableCompilerOption(true);
  const bool compiled = jit->CompileMethod(identity, self, art::CompilationKind::kOptimized, false);
  jit->GetJitCompiler()->SetDebuggableCompilerOption(false);
  const void* entry = identity->GetEntryPointFromQuickCompiledCode();
  if (!compiled || !jit->GetCodeCache()->ContainsPc(entry) ||
      !art::CodeInfo::IsDebuggable(art::OatQuickMethodHeader::FromEntryPoint(entry)->
                                  GetOptimizedCodeInfoPtr())) return false;
  ReferenceExitListener listener(identity, native_identity, expected);
  auto* instrumentation = art::Runtime::Current()->GetInstrumentation();
  const char* key = "darwin-reference-exit-acceptance";
  {
    art::ScopedThreadSuspension suspended(self, art::ThreadState::kSuspended);
    art::gc::ScopedGCCriticalSection gc(self, art::gc::kGcCauseInstrumentation,
                                      art::gc::kCollectorTypeInstrumentation);
    art::ScopedSuspendAll all(key);
    instrumentation->AddListener(&listener, art::instrumentation::Instrumentation::kMethodExited);
    instrumentation->EnableMethodTracing(key, &listener, false);
  }
  bool ok = identity->GetEntryPointFromQuickCompiledCode() == entry;
  for (auto* method : {identity, native_identity}) {
    for (bool is_null : {true, false}) {
      listener.expect_null = is_null;
      uint32_t argument = art::mirror::CompressedReference<art::mirror::Object>::FromMirrorPtr(
          is_null ? nullptr : expected.Get()).AsVRegValue();
      art::JValue result;
      method->Invoke(self, &argument, sizeof(argument), &result, "LL");
      ok &= !self->IsExceptionPending() && result.GetL() == (is_null ? nullptr : expected.Get());
      ok &= identity->GetEntryPointFromQuickCompiledCode() == entry;
      ok &= jit->GetCodeCache()->ContainsPc(entry);
    }
  }
  {
    art::ScopedThreadSuspension suspended(self, art::ThreadState::kSuspended);
    art::gc::ScopedGCCriticalSection gc(self, art::gc::kGcCauseInstrumentation,
                                      art::gc::kCollectorTypeInstrumentation);
    art::ScopedSuspendAll all(key);
    instrumentation->RemoveListener(&listener, art::instrumentation::Instrumentation::kMethodExited);
    instrumentation->DisableMethodTracing(key);
  }
  return ok && listener.ok && listener.calls[0] == 2 && listener.calls[1] == 2 &&
      listener.collections[0] == 1 && listener.collections[1] == 1;
}
}  // namespace aim_jni_acceptance_phase
