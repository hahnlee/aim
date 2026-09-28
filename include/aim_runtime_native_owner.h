#ifndef AIM_RUNTIME_NATIVE_OWNER_H_
#define AIM_RUNTIME_NATIVE_OWNER_H_

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct RuntimeNativeOwner RuntimeNativeOwner;
typedef int32_t (*RuntimeNativeOwnerDropFn)(void* value, void* context);

RuntimeNativeOwner* aim_runtime_native_owner_create(void);
int32_t aim_runtime_native_owner_attach(
    RuntimeNativeOwner* owner,
    uint32_t order,
    void* value,
    void* context,
    RuntimeNativeOwnerDropFn drop_fn);
void* aim_runtime_native_owner_lookup(RuntimeNativeOwner* owner,
                                             uint32_t order);
int32_t aim_runtime_native_owner_destroy(RuntimeNativeOwner* owner);

#ifdef __cplusplus
}
#endif

#endif  // AIM_RUNTIME_NATIVE_OWNER_H_
