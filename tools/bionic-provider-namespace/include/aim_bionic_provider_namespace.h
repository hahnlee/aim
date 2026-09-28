#ifndef AIM_BIONIC_PROVIDER_NAMESPACE_H_
#define AIM_BIONIC_PROVIDER_NAMESPACE_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct AimBionicNamespace AimBionicNamespace;

typedef enum AimBionicProviderId {
  AIM_BIONIC_PROVIDER_LEAF = 0,
  AIM_BIONIC_PROVIDER_ALLOCATOR = 1,
  AIM_BIONIC_PROVIDER_ERRNO = 2,
  AIM_BIONIC_PROVIDER_FILESYSTEM = 3,
  AIM_BIONIC_PROVIDER_TIME = 4,
  AIM_BIONIC_PROVIDER_PTHREAD = 5,
  AIM_BIONIC_PROVIDER_PROCESS_STATE = 6,
  AIM_BIONIC_PROVIDER_PHDR = 7,
  AIM_BIONIC_PROVIDER_STDIO = 8,
  AIM_BIONIC_PROVIDER_LOCALE = 9,
  AIM_BIONIC_PROVIDER_NUMERIC = 10,
  AIM_BIONIC_PROVIDER_FLOAT_CONVERSION = 11,
  AIM_BIONIC_PROVIDER_FORMAT = 12,
  AIM_BIONIC_PROVIDER_STRERROR = 13,
  AIM_BIONIC_PROVIDER_WIDE_INTEGER = 14,
  AIM_BIONIC_PROVIDER_ABORT = 15,
  AIM_BIONIC_PROVIDER_LIBLOG = 16,
  AIM_BIONIC_PROVIDER_DSO_LIFECYCLE = 17,
  AIM_BIONIC_PROVIDER_WIDE_FLOAT = 18,
  AIM_BIONIC_PROVIDER_SYSLOG = 19,
  AIM_BIONIC_PROVIDER_FORMATTED_STDIO = 20,
  AIM_BIONIC_PROVIDER_SYSCALL = 21,
  AIM_BIONIC_PROVIDER_BINARY128_CONVERSION = 22,
  AIM_BIONIC_PROVIDER_WIDE_STDIO = 23,
  AIM_BIONIC_PROVIDER_SCANF = 24,
  AIM_BIONIC_PROVIDER_SWPRINTF = 25,
  AIM_BIONIC_PROVIDER_IOCTL = 26,
  AIM_BIONIC_PROVIDER_STRFTIME = 27,
  AIM_BIONIC_PROVIDER_SENDFILE = 28,
  AIM_BIONIC_PROVIDER_CENTRAL_FD_BROKER = 29,
  AIM_BIONIC_PROVIDER_SOCKET = 30,
  AIM_BIONIC_PROVIDER_DNS = 31,
  AIM_BIONIC_PROVIDER_MATH = 32,
  AIM_BIONIC_PROVIDER_VM = 33,
  AIM_BIONIC_PROVIDER_BINDER_NDK = 34,
  AIM_BIONIC_PROVIDER_AAUDIO = 35,
  AIM_BIONIC_PROVIDER_COUNT = 36,
} AimBionicProviderId;

typedef enum AimBionicNamespaceStatus {
  AIM_BIONIC_NAMESPACE_OK = 0,
  AIM_BIONIC_NAMESPACE_INVALID_ARGUMENT = 1,
  AIM_BIONIC_NAMESPACE_DUPLICATE_BINDING = 2,
  AIM_BIONIC_NAMESPACE_PROVIDER_UNBOUND = 3,
  AIM_BIONIC_NAMESPACE_NOT_SEALED = 4,
  AIM_BIONIC_NAMESPACE_ALREADY_SEALED = 5,
  AIM_BIONIC_NAMESPACE_SHUTTING_DOWN = 6,
  AIM_BIONIC_NAMESPACE_UNKNOWN_SONAME = 7,
  AIM_BIONIC_NAMESPACE_UNKNOWN_VERSION = 8,
  AIM_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL = 9,
  AIM_BIONIC_NAMESPACE_PROVIDER_REJECTED = 10,
} AimBionicNamespaceStatus;

/* Provider resolver callbacks must be thread-safe. The namespace has already
 * checked the exact SONAME, symbol owner, and version before invoking one. */
typedef uintptr_t (*AimBionicProviderResolve)(void *context,
                                                    const char *soname,
                                                    const char *symbol,
                                                    const char *version);
typedef void (*AimBionicProviderRelease)(void *context);

typedef struct AimBionicProviderBinding {
  AimBionicProviderId provider;
  void *context;
  AimBionicProviderResolve resolve;
  AimBionicProviderRelease release;
} AimBionicProviderBinding;

typedef struct AimBionicNamespaceResult {
  AimBionicNamespaceStatus status;
  AimBionicProviderId owner;
  uintptr_t address;
} AimBionicNamespaceResult;

AimBionicNamespace *aim_bionic_namespace_create(void);

/* Binding is allowed only before seal. Every provider in the generated
 * ownership manifest must be bound exactly once before seal succeeds. */
AimBionicNamespaceStatus
aim_bionic_namespace_bind(AimBionicNamespace *namespace_instance,
                                 const AimBionicProviderBinding *binding);
AimBionicNamespaceStatus
aim_bionic_namespace_seal(AimBionicNamespace *namespace_instance);
/* Non-mutating image admission. Requires a sealed live provider set and its
 * declared SONAME; does not resolve a synthetic symbol or acquire a lease. */
AimBionicNamespaceStatus aim_bionic_namespace_image_status(
    AimBionicNamespace*, const char* soname);

/* Exact closed lookup. Version aliases are explicit manifest triples: NDK
 * liblog imports are unversioned while reviewed system/APEX imports may use
 * LIBLOG. No host lookup or fallback occurs. */
AimBionicNamespaceResult aim_bionic_namespace_resolve(
    AimBionicNamespace *namespace_instance, const char *soname,
    const char *symbol, const char *version);

/* Stops admission, waits for in-flight resolver callbacks, and releases all
 * providers once in the documented dependency order. Guest DSO finalizers
 * must already have run before this boundary. */
AimBionicNamespaceStatus aim_bionic_namespace_teardown(
    AimBionicNamespace *namespace_instance);
void aim_bionic_namespace_destroy(
    AimBionicNamespace *namespace_instance);

size_t aim_bionic_namespace_owned_count(void);
size_t aim_bionic_namespace_unsupported_libc_count(void);
/* Returns one and immutable manifest strings for an unsupported libc import;
 * returns zero for an owned or unknown symbol. */
int aim_bionic_namespace_unsupported_libc(const char *symbol,
                                                 char *capability_class,
                                                 const char **reason);
const char *aim_bionic_provider_name(AimBionicProviderId provider);
const char *
aim_bionic_namespace_status_name(AimBionicNamespaceStatus status);

#ifdef __cplusplus
}
#endif

#endif // AIM_BIONIC_PROVIDER_NAMESPACE_H_
