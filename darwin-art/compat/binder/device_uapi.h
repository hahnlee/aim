#pragma once
// Binder commands use Linux encoding; unrelated host ioctls retain Darwin
// encoding. The upstream Binder header declares commands as enum constants,
// so their values survive restoring the host macros after this include.
#include <sys/ioctl.h>
#pragma push_macro("_IOC")
#pragma push_macro("_IO")
#pragma push_macro("_IOR")
#pragma push_macro("_IOW")
#pragma push_macro("_IOWR")
#pragma push_macro("IOC_IN")
#pragma push_macro("IOC_OUT")
#pragma push_macro("IOC_INOUT")
#undef _IOC
#undef _IO
#undef _IOR
#undef _IOW
#undef _IOWR
#undef IOC_IN
#undef IOC_OUT
#undef IOC_INOUT
#include <linux/android/binder.h>
#pragma pop_macro("IOC_INOUT")
#pragma pop_macro("IOC_OUT")
#pragma pop_macro("IOC_IN")
#pragma pop_macro("_IOWR")
#pragma pop_macro("_IOW")
#pragma pop_macro("_IOR")
#pragma pop_macro("_IO")
#pragma pop_macro("_IOC")
