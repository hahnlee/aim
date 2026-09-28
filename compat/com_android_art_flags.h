#pragma once

// Minimal aconfig surface for the interpreter-only Darwin bootstrap. AOSP's
// generated header contains many release flags; this first native runtime gate
// disables optional always-on profiling.
namespace com::android::art::flags {
// Keep one audited optimizing backend; the optional fast baseline backend
// has separate code-generation assumptions and is not enabled on Darwin.
inline bool fast_baseline_compiler() { return false; }

inline bool always_enable_profile_code() {
  return false;
}

}  // namespace com::android::art::flags
