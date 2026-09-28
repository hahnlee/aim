#pragma once

#include "base/globals.h"
#include "aarch64/macro-assembler-aarch64.h"

namespace art::arm64 {

// Host address representation only. HIR values, stack maps, poisoning and
// read barriers remain ART's responsibility. No hidden scratch allocation:
// callers own the destination register for the entire memory operation.
template <uintptr_t Base = kArtCompressedReferenceBase>
struct ReferenceCodegenARM64 {
  static_assert((Base & UINT32_MAX) == 0, "reference window must be 4GiB aligned");
  // A single bit is directly encodable by ORR; arbitrary constants can make
  // VIXL allocate a hidden scratch register when expanding the instruction.
  static_assert(Base == 0 || (Base & (Base - 1)) == 0,
                "reference base must be zero or a single-bit logical immediate");
  static constexpr bool kNeedsHeapBase = Base != 0;

  // Input is native null or an unpoisoned pointer in this reference window.
  // The low-zero offset is reserved for null. Does not alter flags.
  static void Encode(vixl::aarch64::MacroAssembler* assembler,
                     vixl::aarch64::Register destination,
                     vixl::aarch64::Register address) {
    assembler->Uxtw(destination.X(), address.W());
  }

  // Only for an already null-checked heap reference. Does not alter flags.
  // May alias its input; the caller must preserve any still-live HIR value.
  static void DecodeNonNull(vixl::aarch64::MacroAssembler* assembler,
                            vixl::aarch64::Register destination,
                            vixl::aarch64::Register reference) {
    assembler->Uxtw(destination.X(), reference.W());
    if constexpr (Base != 0) {
      assembler->Orr(destination.X(), destination.X(), Base);
    }
  }

  // Quick/JNI reference results expose native pointers, including native null.
  // This operation clobbers condition flags on a nonzero-base host.
  static void DecodeNullable(vixl::aarch64::MacroAssembler* assembler,
                             vixl::aarch64::Register destination,
                             vixl::aarch64::Register reference) {
    assembler->Uxtw(destination.X(), reference.W());
    if constexpr (Base != 0) {
      assembler->Cmp(destination.W(), 0);
      assembler->Orr(destination.X(), destination.X(), Base);
      assembler->Csel(destination.X(), vixl::aarch64::xzr,
                      destination.X(), vixl::aarch64::eq);
    }
  }
};

}  // namespace art::arm64
