#include <stddef.h>

extern int __cxa_atexit(void (*function)(void*), void* argument, void* dso);
extern void __cxa_finalize(void* dso);

typedef int (*CxaAtexitSignature)(void (*)(void*), void*, void*);
typedef void (*CxaFinalizeSignature)(void*);

_Static_assert(_Generic(&__cxa_atexit, CxaAtexitSignature: 1, default: 0),
               "__cxa_atexit signature drift");
_Static_assert(_Generic(&__cxa_finalize, CxaFinalizeSignature: 1, default: 0),
               "__cxa_finalize signature drift");
_Static_assert(sizeof(void (*)(void*)) == sizeof(void*),
               "Android arm64 callback pointer width drift");
