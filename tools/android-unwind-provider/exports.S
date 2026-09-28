.text

.macro AIM_UNWIND_EXPORT name
  .globl \name
  .type \name, %function
  .p2align 2
\name:
  b aim_internal\name
  .size \name, .-\name
.endm

AIM_UNWIND_EXPORT _Unwind_Backtrace
AIM_UNWIND_EXPORT _Unwind_DeleteException
AIM_UNWIND_EXPORT _Unwind_FindEnclosingFunction
AIM_UNWIND_EXPORT _Unwind_Find_FDE
AIM_UNWIND_EXPORT _Unwind_ForcedUnwind
AIM_UNWIND_EXPORT _Unwind_GetCFA
AIM_UNWIND_EXPORT _Unwind_GetDataRelBase
AIM_UNWIND_EXPORT _Unwind_GetGR
AIM_UNWIND_EXPORT _Unwind_GetIP
AIM_UNWIND_EXPORT _Unwind_GetIPInfo
AIM_UNWIND_EXPORT _Unwind_GetLanguageSpecificData
AIM_UNWIND_EXPORT _Unwind_GetRegionStart
AIM_UNWIND_EXPORT _Unwind_GetTextRelBase
AIM_UNWIND_EXPORT _Unwind_RaiseException
AIM_UNWIND_EXPORT _Unwind_Resume
AIM_UNWIND_EXPORT _Unwind_Resume_or_Rethrow
AIM_UNWIND_EXPORT _Unwind_SetGR
AIM_UNWIND_EXPORT _Unwind_SetIP

.section .note.GNU-stack,"",%progbits
