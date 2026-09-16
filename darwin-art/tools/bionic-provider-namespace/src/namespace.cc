#include "darwin_art_bionic_provider_namespace.h"

#include <array>
#include <condition_variable>
#include <cstring>
#include <mutex>
#include <new>

namespace {

struct Ownership {
  const char *soname;
  const char *symbol;
  const char *version;
  DarwinArtBionicProviderId owner;
  bool default_version;
};

struct Unsupported {
  const char *symbol;
  char capability_class;
  const char *reason;
};

constexpr Ownership kOwnership[] = {
#include "ownership.inc"
};

constexpr size_t kUnsupportedLibcCount =
#include "unsupported_count.inc"
    ;

constexpr std::array<Unsupported, kUnsupportedLibcCount> kUnsupported = {{
#include "unsupported.inc"
}};

constexpr const char *kProviderNames[] = {
    "leaf",
    "allocator",
    "errno",
    "filesystem",
    "time",
    "pthread",
    "process-state",
    "phdr",
    "stdio",
    "locale",
    "numeric",
    "float-conversion",
    "format",
    "strerror",
    "wide-integer",
    "abort",
    "liblog",
    "dso-lifecycle",
    "wide-float",
    "syslog",
    "formatted-stdio",
    "syscall",
    "binary128-conversion",
    "wide-stdio",
    "scanf",
    "swprintf",
    "ioctl",
    "strftime",
    "sendfile",
    "central-fd-broker",
    "socket",
    "dns",
    "math",
    "vm",
    "binder-ndk",
    "aaudio",
};
static_assert(sizeof(kProviderNames) / sizeof(kProviderNames[0]) ==
              DARWIN_ART_BIONIC_PROVIDER_COUNT);

/* A release hook must only drop provider-owned host state. Android destructor
 * execution belongs before namespace teardown, while resolution is admitted.
 * Dependants are released before the shared errno/allocator/leaf substrate. */
constexpr DarwinArtBionicProviderId kReleaseOrder[] = {
    DARWIN_ART_BIONIC_PROVIDER_DSO_LIFECYCLE,
    DARWIN_ART_BIONIC_PROVIDER_VM,
    DARWIN_ART_BIONIC_PROVIDER_MATH,
    DARWIN_ART_BIONIC_PROVIDER_DNS,
    DARWIN_ART_BIONIC_PROVIDER_SOCKET,
    DARWIN_ART_BIONIC_PROVIDER_AAUDIO,
    DARWIN_ART_BIONIC_PROVIDER_BINDER_NDK,
    DARWIN_ART_BIONIC_PROVIDER_CENTRAL_FD_BROKER,
    DARWIN_ART_BIONIC_PROVIDER_ABORT,
    DARWIN_ART_BIONIC_PROVIDER_SYSLOG,
    DARWIN_ART_BIONIC_PROVIDER_SYSCALL,
    DARWIN_ART_BIONIC_PROVIDER_SWPRINTF,
    DARWIN_ART_BIONIC_PROVIDER_IOCTL,
    DARWIN_ART_BIONIC_PROVIDER_STRFTIME,
    DARWIN_ART_BIONIC_PROVIDER_SENDFILE,
    DARWIN_ART_BIONIC_PROVIDER_LIBLOG,
    DARWIN_ART_BIONIC_PROVIDER_NUMERIC,
    DARWIN_ART_BIONIC_PROVIDER_BINARY128_CONVERSION,
    DARWIN_ART_BIONIC_PROVIDER_WIDE_FLOAT,
    DARWIN_ART_BIONIC_PROVIDER_FLOAT_CONVERSION,
    DARWIN_ART_BIONIC_PROVIDER_WIDE_INTEGER,
    DARWIN_ART_BIONIC_PROVIDER_STRERROR,
    DARWIN_ART_BIONIC_PROVIDER_WIDE_STDIO,
    DARWIN_ART_BIONIC_PROVIDER_SCANF,
    DARWIN_ART_BIONIC_PROVIDER_FORMATTED_STDIO,
    DARWIN_ART_BIONIC_PROVIDER_FORMAT,
    DARWIN_ART_BIONIC_PROVIDER_LOCALE,
    DARWIN_ART_BIONIC_PROVIDER_STDIO,
    DARWIN_ART_BIONIC_PROVIDER_PHDR,
    DARWIN_ART_BIONIC_PROVIDER_PROCESS_STATE,
    DARWIN_ART_BIONIC_PROVIDER_PTHREAD,
    DARWIN_ART_BIONIC_PROVIDER_TIME,
    DARWIN_ART_BIONIC_PROVIDER_FILESYSTEM,
    DARWIN_ART_BIONIC_PROVIDER_ALLOCATOR,
    DARWIN_ART_BIONIC_PROVIDER_LEAF,
    DARWIN_ART_BIONIC_PROVIDER_ERRNO,
};
static_assert(sizeof(kReleaseOrder) / sizeof(kReleaseOrder[0]) ==
              DARWIN_ART_BIONIC_PROVIDER_COUNT);

enum class State { kOpen, kSealed, kDraining, kDead };

int CompareKey(const Ownership &entry, const char *soname, const char *symbol) {
  int comparison = std::strcmp(entry.soname, soname);
  return comparison != 0 ? comparison : std::strcmp(entry.symbol, symbol);
}

bool VersionMatches(const Ownership &entry, const char *version) {
  if (version == nullptr || version[0] == '\0') return entry.default_version;
  return version != nullptr && std::strcmp(entry.version, version) == 0;
}

struct OwnershipLookup {
  const Ownership *matching_version;
  const Ownership *first_symbol;
};

OwnershipLookup FindOwnership(const char *soname, const char *symbol,
                              const char *version) {
  size_t first = 0;
  size_t count = sizeof(kOwnership) / sizeof(kOwnership[0]);
  while (count != 0) {
    const size_t step = count / 2;
    const size_t current = first + step;
    const int comparison = CompareKey(kOwnership[current], soname, symbol);
    if (comparison < 0) {
      first = current + 1;
      count -= step + 1;
    } else {
      count = step;
    }
  }
  if (first == sizeof(kOwnership) / sizeof(kOwnership[0]) ||
      CompareKey(kOwnership[first], soname, symbol) != 0) {
    return {nullptr, nullptr};
  }
  const Ownership *first_symbol = &kOwnership[first];
  for (size_t index = first;
       index < sizeof(kOwnership) / sizeof(kOwnership[0]) &&
       CompareKey(kOwnership[index], soname, symbol) == 0;
       ++index) {
    if (VersionMatches(kOwnership[index], version))
      return {&kOwnership[index], first_symbol};
  }
  return {nullptr, first_symbol};
}

bool KnownSoname(const char *soname) {
  return std::strcmp(soname, "libc.so") == 0 ||
         std::strcmp(soname, "libdl.so") == 0 ||
         std::strcmp(soname, "liblog.so") == 0 ||
         std::strcmp(soname, "libm.so") == 0 ||
         std::strcmp(soname, "libbinder_ndk.so") == 0 ||
         std::strcmp(soname, "libaaudio.so") == 0 ||
         std::strcmp(soname, "libandroid.so") == 0;
}

DarwinArtBionicNamespaceResult Result(DarwinArtBionicNamespaceStatus status,
                                      DarwinArtBionicProviderId owner,
                                      uintptr_t address = 0) {
  return {status, owner, address};
}

} // namespace

struct DarwinArtBionicNamespace {
  std::mutex mutex;
  std::condition_variable idle;
  std::array<DarwinArtBionicProviderBinding, DARWIN_ART_BIONIC_PROVIDER_COUNT>
      bindings{};
  std::array<bool, DARWIN_ART_BIONIC_PROVIDER_COUNT> bound{};
  State state = State::kOpen;
  size_t in_flight = 0;
};

extern "C" DarwinArtBionicNamespace *darwin_art_bionic_namespace_create() {
  return new (std::nothrow) DarwinArtBionicNamespace();
}

extern "C" DarwinArtBionicNamespaceStatus darwin_art_bionic_namespace_bind(
    DarwinArtBionicNamespace *instance,
    const DarwinArtBionicProviderBinding *binding) {
  if (instance == nullptr || binding == nullptr ||
      binding->resolve == nullptr || binding->provider < 0 ||
      binding->provider >= DARWIN_ART_BIONIC_PROVIDER_COUNT) {
    return DARWIN_ART_BIONIC_NAMESPACE_INVALID_ARGUMENT;
  }
  std::lock_guard<std::mutex> lock(instance->mutex);
  if (instance->state != State::kOpen) {
    return DARWIN_ART_BIONIC_NAMESPACE_ALREADY_SEALED;
  }
  const size_t index = static_cast<size_t>(binding->provider);
  if (instance->bound[index]) {
    return DARWIN_ART_BIONIC_NAMESPACE_DUPLICATE_BINDING;
  }
  instance->bindings[index] = *binding;
  instance->bound[index] = true;
  return DARWIN_ART_BIONIC_NAMESPACE_OK;
}

extern "C" DarwinArtBionicNamespaceStatus
darwin_art_bionic_namespace_seal(DarwinArtBionicNamespace *instance) {
  if (instance == nullptr)
    return DARWIN_ART_BIONIC_NAMESPACE_INVALID_ARGUMENT;
  std::lock_guard<std::mutex> lock(instance->mutex);
  if (instance->state != State::kOpen) {
    return DARWIN_ART_BIONIC_NAMESPACE_ALREADY_SEALED;
  }
  for (bool bound : instance->bound) {
    if (!bound)
      return DARWIN_ART_BIONIC_NAMESPACE_PROVIDER_UNBOUND;
  }
  instance->state = State::kSealed;
  return DARWIN_ART_BIONIC_NAMESPACE_OK;
}

extern "C" DarwinArtBionicNamespaceResult
darwin_art_bionic_namespace_resolve(DarwinArtBionicNamespace *instance,
                                    const char *soname, const char *symbol,
                                    const char *version) {
  if (instance == nullptr || soname == nullptr || symbol == nullptr ||
      soname[0] == '\0' || symbol[0] == '\0') {
    return Result(DARWIN_ART_BIONIC_NAMESPACE_INVALID_ARGUMENT,
                  DARWIN_ART_BIONIC_PROVIDER_COUNT);
  }
  if (!KnownSoname(soname)) {
    return Result(DARWIN_ART_BIONIC_NAMESPACE_UNKNOWN_SONAME,
                  DARWIN_ART_BIONIC_PROVIDER_COUNT);
  }
  const OwnershipLookup lookup = FindOwnership(soname, symbol, version);
  if (lookup.first_symbol == nullptr) {
    return Result(DARWIN_ART_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL,
                  DARWIN_ART_BIONIC_PROVIDER_COUNT);
  }
  if (lookup.matching_version == nullptr) {
    return Result(DARWIN_ART_BIONIC_NAMESPACE_UNKNOWN_VERSION,
                  lookup.first_symbol->owner);
  }
  const Ownership *ownership = lookup.matching_version;

  DarwinArtBionicProviderBinding binding{};
  {
    std::lock_guard<std::mutex> lock(instance->mutex);
    if (instance->state == State::kOpen) {
      return Result(DARWIN_ART_BIONIC_NAMESPACE_NOT_SEALED, ownership->owner);
    }
    if (instance->state != State::kSealed) {
      return Result(DARWIN_ART_BIONIC_NAMESPACE_SHUTTING_DOWN,
                    ownership->owner);
    }
    binding = instance->bindings[static_cast<size_t>(ownership->owner)];
    ++instance->in_flight;
  }

  const uintptr_t address =
      binding.resolve(binding.context, soname, symbol,
                      ownership->version[0] ? ownership->version : nullptr);
  {
    std::lock_guard<std::mutex> lock(instance->mutex);
    --instance->in_flight;
    if (instance->in_flight == 0)
      instance->idle.notify_all();
  }
  if (address == 0) {
    return Result(DARWIN_ART_BIONIC_NAMESPACE_PROVIDER_REJECTED,
                  ownership->owner);
  }
  return Result(DARWIN_ART_BIONIC_NAMESPACE_OK, ownership->owner, address);
}

extern "C" DarwinArtBionicNamespaceStatus darwin_art_bionic_namespace_image_status(
    DarwinArtBionicNamespace* instance, const char* soname) {
  if (!instance || !soname || !*soname) return DARWIN_ART_BIONIC_NAMESPACE_INVALID_ARGUMENT;
  std::lock_guard<std::mutex> lock(instance->mutex);
  if (instance->state != State::kSealed) return DARWIN_ART_BIONIC_NAMESPACE_NOT_SEALED;
  return KnownSoname(soname) ? DARWIN_ART_BIONIC_NAMESPACE_OK
                           : DARWIN_ART_BIONIC_NAMESPACE_UNKNOWN_SONAME;
}

extern "C" DarwinArtBionicNamespaceStatus
darwin_art_bionic_namespace_teardown(DarwinArtBionicNamespace *instance) {
  if (instance == nullptr)
    return DARWIN_ART_BIONIC_NAMESPACE_INVALID_ARGUMENT;
  std::array<DarwinArtBionicProviderBinding, DARWIN_ART_BIONIC_PROVIDER_COUNT>
      bindings{};
  {
    std::unique_lock<std::mutex> lock(instance->mutex);
    if (instance->state == State::kDead) {
      return DARWIN_ART_BIONIC_NAMESPACE_OK;
    }
    if (instance->state == State::kDraining) {
      instance->idle.wait(
          lock, [instance] { return instance->state == State::kDead; });
      return DARWIN_ART_BIONIC_NAMESPACE_OK;
    }
    instance->state = State::kDraining;
    instance->idle.wait(lock, [instance] { return instance->in_flight == 0; });
    bindings = instance->bindings;
  }

  for (DarwinArtBionicProviderId provider : kReleaseOrder) {
    const DarwinArtBionicProviderBinding &binding =
        bindings[static_cast<size_t>(provider)];
    if (binding.release != nullptr)
      binding.release(binding.context);
  }
  {
    std::lock_guard<std::mutex> lock(instance->mutex);
    instance->state = State::kDead;
    instance->idle.notify_all();
  }
  return DARWIN_ART_BIONIC_NAMESPACE_OK;
}

extern "C" void
darwin_art_bionic_namespace_destroy(DarwinArtBionicNamespace *instance) {
  if (instance == nullptr)
    return;
  (void)darwin_art_bionic_namespace_teardown(instance);
  delete instance;
}

extern "C" size_t darwin_art_bionic_namespace_owned_count() {
  return sizeof(kOwnership) / sizeof(kOwnership[0]);
}

extern "C" size_t darwin_art_bionic_namespace_unsupported_libc_count() {
  return kUnsupportedLibcCount;
}

extern "C" int darwin_art_bionic_namespace_unsupported_libc(
    const char *symbol, char *capability_class, const char **reason) {
  if (symbol == nullptr || capability_class == nullptr || reason == nullptr) {
    return 0;
  }
  size_t first = 0;
  size_t count = kUnsupported.size();
  while (count != 0) {
    const size_t step = count / 2;
    const size_t current = first + step;
    const int comparison = std::strcmp(kUnsupported[current].symbol, symbol);
    if (comparison < 0) {
      first = current + 1;
      count -= step + 1;
    } else {
      count = step;
    }
  }
  if (first == kUnsupported.size() ||
      std::strcmp(kUnsupported[first].symbol, symbol) != 0) {
    return 0;
  }
  *capability_class = kUnsupported[first].capability_class;
  *reason = kUnsupported[first].reason;
  return 1;
}

extern "C" const char *
darwin_art_bionic_provider_name(DarwinArtBionicProviderId provider) {
  return provider >= 0 && provider < DARWIN_ART_BIONIC_PROVIDER_COUNT
             ? kProviderNames[static_cast<size_t>(provider)]
             : nullptr;
}

extern "C" const char *
darwin_art_bionic_namespace_status_name(DarwinArtBionicNamespaceStatus status) {
  switch (status) {
  case DARWIN_ART_BIONIC_NAMESPACE_OK:
    return "ok";
  case DARWIN_ART_BIONIC_NAMESPACE_INVALID_ARGUMENT:
    return "invalid-argument";
  case DARWIN_ART_BIONIC_NAMESPACE_DUPLICATE_BINDING:
    return "duplicate-binding";
  case DARWIN_ART_BIONIC_NAMESPACE_PROVIDER_UNBOUND:
    return "provider-unbound";
  case DARWIN_ART_BIONIC_NAMESPACE_NOT_SEALED:
    return "not-sealed";
  case DARWIN_ART_BIONIC_NAMESPACE_ALREADY_SEALED:
    return "already-sealed";
  case DARWIN_ART_BIONIC_NAMESPACE_SHUTTING_DOWN:
    return "shutting-down";
  case DARWIN_ART_BIONIC_NAMESPACE_UNKNOWN_SONAME:
    return "unknown-soname";
  case DARWIN_ART_BIONIC_NAMESPACE_UNKNOWN_VERSION:
    return "unknown-version";
  case DARWIN_ART_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL:
    return "unsupported-symbol";
  case DARWIN_ART_BIONIC_NAMESPACE_PROVIDER_REJECTED:
    return "provider-rejected";
  }
  return "invalid-status";
}
