#ifndef AIM_CPU_FEATURES_MACROS_H_
#define AIM_CPU_FEATURES_MACROS_H_

// ART's cross-ISA feature parsers include this only to guard native ARM32/X86
// detection. aim runs on ARM64; never enable foreign ISA detectors.
#if !defined(__APPLE__) || !defined(__aarch64__)
#error "This CPU feature guard is only supported for Darwin ARM64"
#endif
#define CPU_FEATURES_ARCH_AARCH64 1

#endif
