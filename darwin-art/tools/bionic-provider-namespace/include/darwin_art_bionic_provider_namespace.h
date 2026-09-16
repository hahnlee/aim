#ifndef DARWIN_ART_BIONIC_PROVIDER_NAMESPACE_H_
#define DARWIN_ART_BIONIC_PROVIDER_NAMESPACE_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct DarwinArtBionicNamespace DarwinArtBionicNamespace;

typedef enum DarwinArtBionicProviderId {
  DARWIN_ART_BIONIC_PROVIDER_LEAF = 0,
  DARWIN_ART_BIONIC_PROVIDER_ALLOCATOR = 1,
  DARWIN_ART_BIONIC_PROVIDER_ERRNO = 2,
  DARWIN_ART_BIONIC_PROVIDER_FILESYSTEM = 3,
  DARWIN_ART_BIONIC_PROVIDER_TIME = 4,
  DARWIN_ART_BIONIC_PROVIDER_PTHREAD = 5,
  DARWIN_ART_BIONIC_PROVIDER_PROCESS_STATE = 6,
  DARWIN_ART_BIONIC_PROVIDER_PHDR = 7,
  DARWIN_ART_BIONIC_PROVIDER_STDIO = 8,
  DARWIN_ART_BIONIC_PROVIDER_LOCALE = 9,
  DARWIN_ART_BIONIC_PROVIDER_NUMERIC = 10,
  DARWIN_ART_BIONIC_PROVIDER_FLOAT_CONVERSION = 11,
  DARWIN_ART_BIONIC_PROVIDER_FORMAT = 12,
  DARWIN_ART_BIONIC_PROVIDER_STRERROR = 13,
  DARWIN_ART_BIONIC_PROVIDER_WIDE_INTEGER = 14,
  DARWIN_ART_BIONIC_PROVIDER_ABORT = 15,
  DARWIN_ART_BIONIC_PROVIDER_LIBLOG = 16,
  DARWIN_ART_BIONIC_PROVIDER_DSO_LIFECYCLE = 17,
  DARWIN_ART_BIONIC_PROVIDER_WIDE_FLOAT = 18,
  DARWIN_ART_BIONIC_PROVIDER_SYSLOG = 19,
  DARWIN_ART_BIONIC_PROVIDER_FORMATTED_STDIO = 20,
  DARWIN_ART_BIONIC_PROVIDER_SYSCALL = 21,
  DARWIN_ART_BIONIC_PROVIDER_BINARY128_CONVERSION = 22,
  DARWIN_ART_BIONIC_PROVIDER_WIDE_STDIO = 23,
  DARWIN_ART_BIONIC_PROVIDER_SCANF = 24,
  DARWIN_ART_BIONIC_PROVIDER_SWPRINTF = 25,
  DARWIN_ART_BIONIC_PROVIDER_IOCTL = 26,
  DARWIN_ART_BIONIC_PROVIDER_STRFTIME = 27,
  DARWIN_ART_BIONIC_PROVIDER_SENDFILE = 28,
  DARWIN_ART_BIONIC_PROVIDER_CENTRAL_FD_BROKER = 29,
  DARWIN_ART_BIONIC_PROVIDER_SOCKET = 30,
  DARWIN_ART_BIONIC_PROVIDER_DNS = 31,
  DARWIN_ART_BIONIC_PROVIDER_MATH = 32,
  DARWIN_ART_BIONIC_PROVIDER_VM = 33,
  DARWIN_ART_BIONIC_PROVIDER_BINDER_NDK = 34,
  DARWIN_ART_BIONIC_PROVIDER_AAUDIO = 35,
  DARWIN_ART_BIONIC_PROVIDER_COUNT = 36,
} DarwinArtBionicProviderId;

typedef enum DarwinArtBionicNamespaceStatus {
  DARWIN_ART_BIONIC_NAMESPACE_OK = 0,
  DARWIN_ART_BIONIC_NAMESPACE_INVALID_ARGUMENT = 1,
  DARWIN_ART_BIONIC_NAMESPACE_DUPLICATE_BINDING = 2,
  DARWIN_ART_BIONIC_NAMESPACE_PROVIDER_UNBOUND = 3,
  DARWIN_ART_BIONIC_NAMESPACE_NOT_SEALED = 4,
  DARWIN_ART_BIONIC_NAMESPACE_ALREADY_SEALED = 5,
  DARWIN_ART_BIONIC_NAMESPACE_SHUTTING_DOWN = 6,
  DARWIN_ART_BIONIC_NAMESPACE_UNKNOWN_SONAME = 7,
  DARWIN_ART_BIONIC_NAMESPACE_UNKNOWN_VERSION = 8,
  DARWIN_ART_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL = 9,
  DARWIN_ART_BIONIC_NAMESPACE_PROVIDER_REJECTED = 10,
} DarwinArtBionicNamespaceStatus;

/* Provider resolver callbacks must be thread-safe. The namespace has already
 * checked the exact SONAME, symbol owner, and version before invoking one. */
typedef uintptr_t (*DarwinArtBionicProviderResolve)(void *context,
                                                    const char *soname,
                                                    const char *symbol,
                                                    const char *version);
typedef void (*DarwinArtBionicProviderRelease)(void *context);

typedef struct DarwinArtBionicProviderBinding {
  DarwinArtBionicProviderId provider;
  void *context;
  DarwinArtBionicProviderResolve resolve;
  DarwinArtBionicProviderRelease release;
} DarwinArtBionicProviderBinding;

typedef struct DarwinArtBionicNamespaceResult {
  DarwinArtBionicNamespaceStatus status;
  DarwinArtBionicProviderId owner;
  uintptr_t address;
} DarwinArtBionicNamespaceResult;

DarwinArtBionicNamespace *darwin_art_bionic_namespace_create(void);

/* Binding is allowed only before seal. Every provider in the generated
 * ownership manifest must be bound exactly once before seal succeeds. */
DarwinArtBionicNamespaceStatus
darwin_art_bionic_namespace_bind(DarwinArtBionicNamespace *namespace_instance,
                                 const DarwinArtBionicProviderBinding *binding);
DarwinArtBionicNamespaceStatus
darwin_art_bionic_namespace_seal(DarwinArtBionicNamespace *namespace_instance);
/* Non-mutating image admission. Requires a sealed live provider set and its
 * declared SONAME; does not resolve a synthetic symbol or acquire a lease. */
DarwinArtBionicNamespaceStatus darwin_art_bionic_namespace_image_status(
    DarwinArtBionicNamespace*, const char* soname);

/* Exact closed lookup. Version aliases are explicit manifest triples: NDK
 * liblog imports are unversioned while reviewed system/APEX imports may use
 * LIBLOG. No host lookup or fallback occurs. */
DarwinArtBionicNamespaceResult darwin_art_bionic_namespace_resolve(
    DarwinArtBionicNamespace *namespace_instance, const char *soname,
    const char *symbol, const char *version);

/* Stops admission, waits for in-flight resolver callbacks, and releases all
 * providers once in the documented dependency order. Guest DSO finalizers
 * must already have run before this boundary. */
DarwinArtBionicNamespaceStatus darwin_art_bionic_namespace_teardown(
    DarwinArtBionicNamespace *namespace_instance);
void darwin_art_bionic_namespace_destroy(
    DarwinArtBionicNamespace *namespace_instance);

size_t darwin_art_bionic_namespace_owned_count(void);
size_t darwin_art_bionic_namespace_unsupported_libc_count(void);
/* Returns one and immutable manifest strings for an unsupported libc import;
 * returns zero for an owned or unknown symbol. */
int darwin_art_bionic_namespace_unsupported_libc(const char *symbol,
                                                 char *capability_class,
                                                 const char **reason);
const char *darwin_art_bionic_provider_name(DarwinArtBionicProviderId provider);
const char *
darwin_art_bionic_namespace_status_name(DarwinArtBionicNamespaceStatus status);

#ifdef __cplusplus
}
#endif

#endif // DARWIN_ART_BIONIC_PROVIDER_NAMESPACE_H_
