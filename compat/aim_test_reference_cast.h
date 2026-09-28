#pragma once

#include <type_traits>

// Pre-include every header used by the two pinned ART vreg tests before the
// source-level reinterpret_cast adapter below becomes visible. This confines
// the adapter to expressions in the test translation unit itself.
#include "arch/context.h"
#include "art_method-inl.h"
#include "jni.h"
#include "mirror/object_reference.h"
#include "oat/oat_quick_method_header.h"
#include "scoped_thread_state_change-inl.h"
#include "stack.h"
#include "thread.h"

namespace art {

// Android places the managed heap below 4 GiB, so ART's 454/461 native test
// helpers can cast a 32-bit reference vreg directly to mirror::Object*. Darwin
// keeps the identical 32-bit managed representation as an offset from
// kArtCompressedReferenceBase. Decode only that private test-source boundary;
// all other casts retain their C++ meaning.
template <typename Target, typename Source>
inline Target AimTestReferenceCast(Source value)
    REQUIRES_SHARED(Locks::mutator_lock_) {
  if constexpr (std::is_same_v<Target, mirror::Object*> &&
                std::is_integral_v<Source> && sizeof(Source) <= sizeof(uint32_t)) {
    return mirror::CompressedReference<mirror::Object>::
        FromVRegValue(static_cast<uint32_t>(value)).AsMirrorPtr();
  } else {
    return reinterpret_cast<Target>(value);
  }
}

}  // namespace art

#define reinterpret_cast ::art::AimTestReferenceCast
