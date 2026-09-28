#include "art_reference_codegen_arm64.h"
#include "darwin_jit_memory.h"
#include <cassert>
#include <cstdio>
#include <cstring>
#include <sys/mman.h>
#include <unistd.h>

using namespace vixl::aarch64;

template <uintptr_t Base>
void Check(void* code) {
  using Policy = art::arm64::ReferenceCodegenARM64<Base>;
  const uint32_t values[] = {0, 1, 0x7fffffff, 0x80000000, 0xffffffff};
  for (int mode = 0; mode < 3; ++mode) {
    MacroAssembler assembler;
    assembler.Cmp(wzr, wzr);
    if (mode == 0) Policy::DecodeNullable(&assembler, x0, w0);
    if (mode == 1) Policy::DecodeNonNull(&assembler, x0, w0);
    if (mode == 2) Policy::Encode(&assembler, x0, x0);
    assembler.Mrs(x2, NZCV);
    assembler.Str(x2, MemOperand(x1));
    assembler.Ret();
    assembler.FinalizeCode();
    const size_t size = assembler.GetSizeOfCodeGenerated();
    assert(size < static_cast<size_t>(getpagesize()));
    {
      AimJitWriteScope write;
      memcpy(code, assembler.GetBuffer()->template GetStartAddress<void*>(), size);
      AimFlushJitCode(code, size);
    }
    auto function = reinterpret_cast<uint64_t (*)(uint64_t, uint64_t*)>(code);
    for (uint32_t value : values) {
      if (mode == 1 && value == 0) continue;  // Precondition: nonnull heap object.
      const uint64_t address = value == 0 ? 0 : Base + value;
      uint64_t flags = 0;
      assert(function(mode == 2 ? address : value, &flags) ==
             (mode == 2 ? value : address));
      if (mode != 0) assert(flags == 0x60000000);  // N=0 Z=1 C=1 V=0.
    }
  }
}

int main() {
  const size_t size = static_cast<size_t>(getpagesize());
  void* code = AimMapJitCode(size);
  assert(code != MAP_FAILED);
  Check<0>(code);  // AOSP zero-base representation.
  Check<art::kArtCompressedReferenceBase>(code);
  munmap(code, size);
  puts("reference codegen PASS: zero/high base, null/sign boundaries, flag preservation");
}
