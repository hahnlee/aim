#ifndef AIM_BIONIC_MATH_H_
#define AIM_BIONIC_MATH_H_
#ifdef __cplusplus
extern "C" {
#endif
void* aim_bionic_math_resolve(const char* soname, const char* symbol,
                                     const char* version);
#ifdef __cplusplus
}
#endif
#endif
