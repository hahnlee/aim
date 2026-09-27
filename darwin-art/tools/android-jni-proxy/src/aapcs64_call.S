.text
.p2align 2

// Android arm64's direct JNI CallVoidMethod entry is variadic.  Darwin and
// Android do not share a C va_list ABI, so capture the AAPCS64 argument banks
// before entering Darwin C and build the Android va_list there.
.globl _darwin_art_jni_proxy_call_void_method
_darwin_art_jni_proxy_call_void_method:
  sub sp, sp, #224
  stp x0, x1, [sp, #0]
  stp x2, x3, [sp, #16]
  stp x4, x5, [sp, #32]
  stp x6, x7, [sp, #48]
  stp q0, q1, [sp, #64]
  stp q2, q3, [sp, #96]
  stp q4, q5, [sp, #128]
  stp q6, q7, [sp, #160]
  str x30, [sp, #192]
  ldp x0, x1, [sp, #0]
  ldr x2, [sp, #16]
  add x3, sp, #0
  add x4, sp, #64
  add x5, sp, #224
  bl _darwin_art_jni_proxy_call_void_method_captured
  ldr x30, [sp, #192]
  add sp, sp, #224
  ret

.globl _darwin_art_jni_proxy_call_int_method
_darwin_art_jni_proxy_call_int_method:
  sub sp, sp, #224
  stp x0, x1, [sp, #0]
  stp x2, x3, [sp, #16]
  stp x4, x5, [sp, #32]
  stp x6, x7, [sp, #48]
  stp q0, q1, [sp, #64]
  stp q2, q3, [sp, #96]
  stp q4, q5, [sp, #128]
  stp q6, q7, [sp, #160]
  str x30, [sp, #192]
  ldp x0, x1, [sp, #0]
  ldr x2, [sp, #16]
  add x3, sp, #0
  add x4, sp, #64
  add x5, sp, #224
  bl _darwin_art_jni_proxy_call_int_method_captured
  ldr x30, [sp, #192]
  add sp, sp, #224
  ret

.globl _darwin_art_jni_proxy_call_object_method
_darwin_art_jni_proxy_call_object_method:
  sub sp, sp, #224
  stp x0, x1, [sp, #0]
  stp x2, x3, [sp, #16]
  stp x4, x5, [sp, #32]
  stp x6, x7, [sp, #48]
  stp q0, q1, [sp, #64]
  stp q2, q3, [sp, #96]
  stp q4, q5, [sp, #128]
  stp q6, q7, [sp, #160]
  str x30, [sp, #192]
  ldp x0, x1, [sp, #0]
  ldr x2, [sp, #16]
  add x3, sp, #0
  add x4, sp, #64
  add x5, sp, #224
  bl _darwin_art_jni_proxy_call_object_method_captured
  ldr x30, [sp, #192]
  add sp, sp, #224
  ret

.globl _darwin_art_jni_proxy_new_object
_darwin_art_jni_proxy_new_object:
  sub sp, sp, #224
  stp x0, x1, [sp, #0]
  stp x2, x3, [sp, #16]
  stp x4, x5, [sp, #32]
  stp x6, x7, [sp, #48]
  stp q0, q1, [sp, #64]
  stp q2, q3, [sp, #96]
  stp q4, q5, [sp, #128]
  stp q6, q7, [sp, #160]
  str x30, [sp, #192]
  ldp x0, x1, [sp, #0]
  ldr x2, [sp, #16]
  add x3, sp, #0
  add x4, sp, #64
  add x5, sp, #224
  bl _darwin_art_jni_proxy_new_object_captured
  ldr x30, [sp, #192]
  add sp, sp, #224
  ret

// Static direct-varargs JNI has the same AAPCS64 register layout (JNIEnv*,
// jclass, jmethodID, ...), but must dispatch with the static-call flag.
.globl _darwin_art_jni_proxy_call_static_void_method
_darwin_art_jni_proxy_call_static_void_method:
  sub sp, sp, #224
  stp x0, x1, [sp, #0]
  stp x2, x3, [sp, #16]
  stp x4, x5, [sp, #32]
  stp x6, x7, [sp, #48]
  stp q0, q1, [sp, #64]
  stp q2, q3, [sp, #96]
  stp q4, q5, [sp, #128]
  stp q6, q7, [sp, #160]
  str x30, [sp, #192]
  ldp x0, x1, [sp, #0]
  ldr x2, [sp, #16]
  add x3, sp, #0
  add x4, sp, #64
  add x5, sp, #224
  bl _darwin_art_jni_proxy_call_static_void_method_captured
  ldr x30, [sp, #192]
  add sp, sp, #224
  ret

// Same capture for jint-returning static calls. The Darwin helper returns the
// backend's 32-bit result in w0, which is also the Android AAPCS64 jint ABI.
.globl _darwin_art_jni_proxy_call_static_int_method
_darwin_art_jni_proxy_call_static_int_method:
  sub sp, sp, #224
  stp x0, x1, [sp, #0]
  stp x2, x3, [sp, #16]
  stp x4, x5, [sp, #32]
  stp x6, x7, [sp, #48]
  stp q0, q1, [sp, #64]
  stp q2, q3, [sp, #96]
  stp q4, q5, [sp, #128]
  stp q6, q7, [sp, #160]
  str x30, [sp, #192]
  ldp x0, x1, [sp, #0]
  ldr x2, [sp, #16]
  add x3, sp, #0
  add x4, sp, #64
  add x5, sp, #224
  bl _darwin_art_jni_proxy_call_static_int_method_captured
  ldr x30, [sp, #192]
  add sp, sp, #224
  ret

.globl _darwin_art_jni_proxy_call_static_object_method
_darwin_art_jni_proxy_call_static_object_method:
  sub sp, sp, #224
  stp x0, x1, [sp, #0]
  stp x2, x3, [sp, #16]
  stp x4, x5, [sp, #32]
  stp x6, x7, [sp, #48]
  stp q0, q1, [sp, #64]
  stp q2, q3, [sp, #96]
  stp q4, q5, [sp, #128]
  stp q6, q7, [sp, #160]
  str x30, [sp, #192]
  ldp x0, x1, [sp, #0]
  ldr x2, [sp, #16]
  add x3, sp, #0
  add x4, sp, #64
  add x5, sp, #224
  bl _darwin_art_jni_proxy_call_static_object_method_captured
  ldr x30, [sp, #192]
  add sp, sp, #224
  ret

// The remaining variadic Call<Type>Method entries share one capture shape:
// three named arguments (JNIEnv*, receiver, jmethodID), then the variadic
// AAPCS64 banks and the caller's stack arguments.
.macro DARWIN_ART_JNI_CAPTURE3 name
.globl _\name
_\name:
  sub sp, sp, #224
  stp x0, x1, [sp, #0]
  stp x2, x3, [sp, #16]
  stp x4, x5, [sp, #32]
  stp x6, x7, [sp, #48]
  stp q0, q1, [sp, #64]
  stp q2, q3, [sp, #96]
  stp q4, q5, [sp, #128]
  stp q6, q7, [sp, #160]
  str x30, [sp, #192]
  ldp x0, x1, [sp, #0]
  ldr x2, [sp, #16]
  add x3, sp, #0
  add x4, sp, #64
  add x5, sp, #224
  bl _\name\()_captured
  ldr x30, [sp, #192]
  add sp, sp, #224
  ret
.endm

// CallNonvirtual<Type>Method has four named arguments (JNIEnv*, object,
// jclass, jmethodID); its variadic arguments start at x4.
.macro DARWIN_ART_JNI_CAPTURE4 name
.globl _\name
_\name:
  sub sp, sp, #224
  stp x0, x1, [sp, #0]
  stp x2, x3, [sp, #16]
  stp x4, x5, [sp, #32]
  stp x6, x7, [sp, #48]
  stp q0, q1, [sp, #64]
  stp q2, q3, [sp, #96]
  stp q4, q5, [sp, #128]
  stp q6, q7, [sp, #160]
  str x30, [sp, #192]
  ldp x0, x1, [sp, #0]
  ldp x2, x3, [sp, #16]
  add x4, sp, #0
  add x5, sp, #64
  add x6, sp, #224
  bl _\name\()_captured
  ldr x30, [sp, #192]
  add sp, sp, #224
  ret
.endm

DARWIN_ART_JNI_CAPTURE3 darwin_art_jni_proxy_call_boolean_method
DARWIN_ART_JNI_CAPTURE3 darwin_art_jni_proxy_call_static_boolean_method
DARWIN_ART_JNI_CAPTURE3 darwin_art_jni_proxy_call_byte_method
DARWIN_ART_JNI_CAPTURE3 darwin_art_jni_proxy_call_static_byte_method
DARWIN_ART_JNI_CAPTURE3 darwin_art_jni_proxy_call_char_method
DARWIN_ART_JNI_CAPTURE3 darwin_art_jni_proxy_call_static_char_method
DARWIN_ART_JNI_CAPTURE3 darwin_art_jni_proxy_call_short_method
DARWIN_ART_JNI_CAPTURE3 darwin_art_jni_proxy_call_static_short_method
DARWIN_ART_JNI_CAPTURE3 darwin_art_jni_proxy_call_long_method
DARWIN_ART_JNI_CAPTURE3 darwin_art_jni_proxy_call_static_long_method
DARWIN_ART_JNI_CAPTURE3 darwin_art_jni_proxy_call_float_method
DARWIN_ART_JNI_CAPTURE3 darwin_art_jni_proxy_call_static_float_method
DARWIN_ART_JNI_CAPTURE3 darwin_art_jni_proxy_call_double_method
DARWIN_ART_JNI_CAPTURE3 darwin_art_jni_proxy_call_static_double_method
DARWIN_ART_JNI_CAPTURE4 darwin_art_jni_proxy_call_nonvirtual_object_method
DARWIN_ART_JNI_CAPTURE4 darwin_art_jni_proxy_call_nonvirtual_boolean_method
DARWIN_ART_JNI_CAPTURE4 darwin_art_jni_proxy_call_nonvirtual_byte_method
DARWIN_ART_JNI_CAPTURE4 darwin_art_jni_proxy_call_nonvirtual_char_method
DARWIN_ART_JNI_CAPTURE4 darwin_art_jni_proxy_call_nonvirtual_short_method
DARWIN_ART_JNI_CAPTURE4 darwin_art_jni_proxy_call_nonvirtual_int_method
DARWIN_ART_JNI_CAPTURE4 darwin_art_jni_proxy_call_nonvirtual_long_method
DARWIN_ART_JNI_CAPTURE4 darwin_art_jni_proxy_call_nonvirtual_float_method
DARWIN_ART_JNI_CAPTURE4 darwin_art_jni_proxy_call_nonvirtual_double_method
DARWIN_ART_JNI_CAPTURE4 darwin_art_jni_proxy_call_nonvirtual_void_method
