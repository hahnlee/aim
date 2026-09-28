#pragma once

// Darwin has no system <elf.h>, so the build uses the NDK's ABI definitions.
// linux/elf.h aliases ELF{32,64}_ST_* through the generic ELF_ST_* names,
// while ART's elf_utils.h intentionally reuses those generic names for the
// selected ELF class. Make the class-specific forms self-contained, matching
// the conventional glibc definitions, to avoid a macro recursion.
#if !defined(AIM_PREINCLUDED_ELF)
#include_next <elf.h>
#endif

#undef ELF32_ST_BIND
#undef ELF32_ST_TYPE
#undef ELF64_ST_BIND
#undef ELF64_ST_TYPE
#define ELF32_ST_BIND(value) ((unsigned char)(value) >> 4)
#define ELF32_ST_TYPE(value) ((value) & 0x0f)
#define ELF64_ST_BIND(value) ((unsigned char)(value) >> 4)
#define ELF64_ST_TYPE(value) ((value) & 0x0f)
