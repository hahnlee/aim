#ifndef AIM_BIONIC_PTHREAD_H_
#define AIM_BIONIC_PTHREAD_H_

#include <stddef.h>
#include <stdint.h>
#include <sched.h>
#include <time.h>

#ifdef __cplusplus
extern "C" {
#endif

// Android arm64 NDK object representations. They are transport storage only;
// the provider never casts one to a Darwin pthread object.
typedef int64_t AimAndroidPthread;
typedef int32_t AimAndroidPthreadKey;
typedef int32_t AimAndroidPthreadOnce;
typedef int64_t AimAndroidPthreadMutexAttr;
typedef int64_t AimAndroidPthreadRwlockAttr;
typedef struct AimAndroidPthreadAttr {
  uint32_t flags;
  void* stack_base;
  size_t stack_size;
  size_t guard_size;
  int32_t sched_policy;
  int32_t sched_priority;
  unsigned char reserved[16];
} AimAndroidPthreadAttr;
typedef struct AimAndroidPthreadMutex {
  int32_t opaque[10];
} AimAndroidPthreadMutex;
typedef struct AimAndroidPthreadCond {
  int32_t opaque[12];
} AimAndroidPthreadCond;
typedef struct AimAndroidPthreadRwlock {
  int32_t opaque[14];
} AimAndroidPthreadRwlock;

typedef void (*AimAndroidTlsDestructor)(void* value);
typedef void (*AimAndroidOnceRoutine)(void);
typedef void (*AimAndroidForkRoutine)(void);
typedef void* (*AimAndroidThreadRoutine)(void* argument);

AimAndroidPthread aim_bionic_pthread_self(void);
// Coherent lifecycle owner seam. Attributes use the Android arm64 transport
// layout above; returned tokens are the sole valid inputs to join/detach.
int aim_bionic_pthread_create(
    AimAndroidPthread* thread,
    const void* android_attributes,
    AimAndroidThreadRoutine routine,
    void* argument);
int aim_bionic_pthread_join(AimAndroidPthread thread,
                                   void** return_value);
int aim_bionic_pthread_detach(AimAndroidPthread thread);
int aim_bionic_pthread_getattr_np(
    AimAndroidPthread thread, AimAndroidPthreadAttr* attr);
int aim_bionic_pthread_getschedparam(
    AimAndroidPthread thread, int* policy, struct sched_param* param);
int aim_bionic_pthread_kill(AimAndroidPthread thread,
                                   int signal_number);
int aim_bionic_pthread_attr_init(AimAndroidPthreadAttr* attr);
int aim_bionic_pthread_attr_destroy(AimAndroidPthreadAttr* attr);
int aim_bionic_pthread_key_create(
    AimAndroidPthreadKey* key,
    AimAndroidTlsDestructor destructor);
int aim_bionic_pthread_key_delete(AimAndroidPthreadKey key);
void* aim_bionic_pthread_getspecific(AimAndroidPthreadKey key);
int aim_bionic_pthread_setspecific(AimAndroidPthreadKey key,
                                          const void* value);
int aim_bionic_pthread_once(AimAndroidPthreadOnce* once,
                                   AimAndroidOnceRoutine routine);
int aim_bionic_pthread_atfork(AimAndroidForkRoutine prepare,
                                     AimAndroidForkRoutine parent,
                                     AimAndroidForkRoutine child);
int aim_bionic_pthread_mutexattr_init(
    AimAndroidPthreadMutexAttr* attributes);
int aim_bionic_pthread_mutexattr_destroy(
    AimAndroidPthreadMutexAttr* attributes);
int aim_bionic_pthread_mutexattr_settype(
    AimAndroidPthreadMutexAttr* attributes,
    int type);
int aim_bionic_pthread_mutex_init(AimAndroidPthreadMutex* mutex,
    const AimAndroidPthreadMutexAttr* android_attributes);
int aim_bionic_pthread_mutex_lock(AimAndroidPthreadMutex* mutex);
int aim_bionic_pthread_mutex_trylock(AimAndroidPthreadMutex* mutex);
int aim_bionic_pthread_mutex_unlock(AimAndroidPthreadMutex* mutex);
int aim_bionic_pthread_mutex_destroy(AimAndroidPthreadMutex* mutex);
int aim_bionic_pthread_condattr_init(uint32_t* attributes);
int aim_bionic_pthread_condattr_destroy(uint32_t* attributes);
int aim_bionic_pthread_condattr_setclock(uint32_t* attributes,
                                                int android_clock_id);
int aim_bionic_pthread_cond_init(AimAndroidPthreadCond* cond,
                                        const void* attributes);
int aim_bionic_pthread_cond_wait(AimAndroidPthreadCond* cond,
                                        AimAndroidPthreadMutex* mutex);
int aim_bionic_pthread_cond_timedwait(
    AimAndroidPthreadCond* cond,
    AimAndroidPthreadMutex* mutex,
    const struct timespec* absolute_timeout);
int aim_bionic_pthread_cond_signal(AimAndroidPthreadCond* cond);
int aim_bionic_pthread_cond_broadcast(AimAndroidPthreadCond* cond);
int aim_bionic_pthread_cond_destroy(AimAndroidPthreadCond* cond);
int aim_bionic_pthread_rwlock_rdlock(
    AimAndroidPthreadRwlock* rwlock);
int aim_bionic_pthread_rwlock_wrlock(
    AimAndroidPthreadRwlock* rwlock);
int aim_bionic_pthread_rwlock_unlock(
    AimAndroidPthreadRwlock* rwlock);
int aim_bionic_pthread_rwlock_init(
    AimAndroidPthreadRwlock* rwlock,
    const AimAndroidPthreadRwlockAttr* attributes);
int aim_bionic_pthread_rwlock_destroy(
    AimAndroidPthreadRwlock* rwlock);

// Closed libc.so/LIBC resolver. Unknown SONAME, version, or symbol is rejected.
void* aim_bionic_pthread_resolve(const char* soname,
                                        const char* symbol,
                                        const char* version);

// Test/process-shutdown boundary. It succeeds only when no owned thread,
// no non-current foreign thread, TLS-key, or live synchronization object
// remains; the current foreign identity may be retired at reset.
int aim_bionic_pthread_provider_reset(void);
size_t aim_bionic_pthread_provider_retired_cell_count(void);

// Returns 1 only for implemented capability names. Unsupported names are
// explicit and never forwarded to a Darwin symbol with a similar spelling.
int aim_bionic_pthread_capability(const char* capability);

#ifdef __cplusplus
}
#endif

#endif  // AIM_BIONIC_PTHREAD_H_
