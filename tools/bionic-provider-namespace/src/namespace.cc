#include "aim_bionic_provider_namespace.h"

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
  AimBionicProviderId owner;
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
              AIM_BIONIC_PROVIDER_COUNT);

/* A release hook must only drop provider-owned host state. Android destructor
 * execution belongs before namespace teardown, while resolution is admitted.
 * Dependants are released before the shared errno/allocator/leaf substrate. */
constexpr AimBionicProviderId kReleaseOrder[] = {
    AIM_BIONIC_PROVIDER_DSO_LIFECYCLE,
    AIM_BIONIC_PROVIDER_VM,
    AIM_BIONIC_PROVIDER_MATH,
    AIM_BIONIC_PROVIDER_DNS,
    AIM_BIONIC_PROVIDER_SOCKET,
    AIM_BIONIC_PROVIDER_AAUDIO,
    AIM_BIONIC_PROVIDER_BINDER_NDK,
    AIM_BIONIC_PROVIDER_CENTRAL_FD_BROKER,
    AIM_BIONIC_PROVIDER_ABORT,
    AIM_BIONIC_PROVIDER_SYSLOG,
    AIM_BIONIC_PROVIDER_SYSCALL,
    AIM_BIONIC_PROVIDER_SWPRINTF,
    AIM_BIONIC_PROVIDER_IOCTL,
    AIM_BIONIC_PROVIDER_STRFTIME,
    AIM_BIONIC_PROVIDER_SENDFILE,
    AIM_BIONIC_PROVIDER_LIBLOG,
    AIM_BIONIC_PROVIDER_NUMERIC,
    AIM_BIONIC_PROVIDER_BINARY128_CONVERSION,
    AIM_BIONIC_PROVIDER_WIDE_FLOAT,
    AIM_BIONIC_PROVIDER_FLOAT_CONVERSION,
    AIM_BIONIC_PROVIDER_WIDE_INTEGER,
    AIM_BIONIC_PROVIDER_STRERROR,
    AIM_BIONIC_PROVIDER_WIDE_STDIO,
    AIM_BIONIC_PROVIDER_SCANF,
    AIM_BIONIC_PROVIDER_FORMATTED_STDIO,
    AIM_BIONIC_PROVIDER_FORMAT,
    AIM_BIONIC_PROVIDER_LOCALE,
    AIM_BIONIC_PROVIDER_STDIO,
    AIM_BIONIC_PROVIDER_PHDR,
    AIM_BIONIC_PROVIDER_PROCESS_STATE,
    AIM_BIONIC_PROVIDER_PTHREAD,
    AIM_BIONIC_PROVIDER_TIME,
    AIM_BIONIC_PROVIDER_FILESYSTEM,
    AIM_BIONIC_PROVIDER_ALLOCATOR,
    AIM_BIONIC_PROVIDER_LEAF,
    AIM_BIONIC_PROVIDER_ERRNO,
};
static_assert(sizeof(kReleaseOrder) / sizeof(kReleaseOrder[0]) ==
              AIM_BIONIC_PROVIDER_COUNT);

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

AimBionicNamespaceResult Result(AimBionicNamespaceStatus status,
                                      AimBionicProviderId owner,
                                      uintptr_t address = 0) {
  return {status, owner, address};
}

} // namespace

struct AimBionicNamespace {
  std::mutex mutex;
  std::condition_variable idle;
  std::array<AimBionicProviderBinding, AIM_BIONIC_PROVIDER_COUNT>
      bindings{};
  std::array<bool, AIM_BIONIC_PROVIDER_COUNT> bound{};
  State state = State::kOpen;
  size_t in_flight = 0;
};

extern "C" AimBionicNamespace *aim_bionic_namespace_create() {
  return new (std::nothrow) AimBionicNamespace();
}

extern "C" AimBionicNamespaceStatus aim_bionic_namespace_bind(
    AimBionicNamespace *instance,
    const AimBionicProviderBinding *binding) {
  if (instance == nullptr || binding == nullptr ||
      binding->resolve == nullptr || binding->provider < 0 ||
      binding->provider >= AIM_BIONIC_PROVIDER_COUNT) {
    return AIM_BIONIC_NAMESPACE_INVALID_ARGUMENT;
  }
  std::lock_guard<std::mutex> lock(instance->mutex);
  if (instance->state != State::kOpen) {
    return AIM_BIONIC_NAMESPACE_ALREADY_SEALED;
  }
  const size_t index = static_cast<size_t>(binding->provider);
  if (instance->bound[index]) {
    return AIM_BIONIC_NAMESPACE_DUPLICATE_BINDING;
  }
  instance->bindings[index] = *binding;
  instance->bound[index] = true;
  return AIM_BIONIC_NAMESPACE_OK;
}

extern "C" AimBionicNamespaceStatus
aim_bionic_namespace_seal(AimBionicNamespace *instance) {
  if (instance == nullptr)
    return AIM_BIONIC_NAMESPACE_INVALID_ARGUMENT;
  std::lock_guard<std::mutex> lock(instance->mutex);
  if (instance->state != State::kOpen) {
    return AIM_BIONIC_NAMESPACE_ALREADY_SEALED;
  }
  for (bool bound : instance->bound) {
    if (!bound)
      return AIM_BIONIC_NAMESPACE_PROVIDER_UNBOUND;
  }
  instance->state = State::kSealed;
  return AIM_BIONIC_NAMESPACE_OK;
}

extern "C" AimBionicNamespaceResult
aim_bionic_namespace_resolve(AimBionicNamespace *instance,
                                    const char *soname, const char *symbol,
                                    const char *version) {
  if (instance == nullptr || soname == nullptr || symbol == nullptr ||
      soname[0] == '\0' || symbol[0] == '\0') {
    return Result(AIM_BIONIC_NAMESPACE_INVALID_ARGUMENT,
                  AIM_BIONIC_PROVIDER_COUNT);
  }
  if (!KnownSoname(soname)) {
    return Result(AIM_BIONIC_NAMESPACE_UNKNOWN_SONAME,
                  AIM_BIONIC_PROVIDER_COUNT);
  }
  const OwnershipLookup lookup = FindOwnership(soname, symbol, version);
  if (lookup.first_symbol == nullptr) {
    return Result(AIM_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL,
                  AIM_BIONIC_PROVIDER_COUNT);
  }
  if (lookup.matching_version == nullptr) {
    return Result(AIM_BIONIC_NAMESPACE_UNKNOWN_VERSION,
                  lookup.first_symbol->owner);
  }
  const Ownership *ownership = lookup.matching_version;

  AimBionicProviderBinding binding{};
  {
    std::lock_guard<std::mutex> lock(instance->mutex);
    if (instance->state == State::kOpen) {
      return Result(AIM_BIONIC_NAMESPACE_NOT_SEALED, ownership->owner);
    }
    if (instance->state != State::kSealed) {
      return Result(AIM_BIONIC_NAMESPACE_SHUTTING_DOWN,
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
    return Result(AIM_BIONIC_NAMESPACE_PROVIDER_REJECTED,
                  ownership->owner);
  }
  return Result(AIM_BIONIC_NAMESPACE_OK, ownership->owner, address);
}

extern "C" AimBionicNamespaceStatus aim_bionic_namespace_image_status(
    AimBionicNamespace* instance, const char* soname) {
  if (!instance || !soname || !*soname) return AIM_BIONIC_NAMESPACE_INVALID_ARGUMENT;
  std::lock_guard<std::mutex> lock(instance->mutex);
  if (instance->state != State::kSealed) return AIM_BIONIC_NAMESPACE_NOT_SEALED;
  return KnownSoname(soname) ? AIM_BIONIC_NAMESPACE_OK
                           : AIM_BIONIC_NAMESPACE_UNKNOWN_SONAME;
}

extern "C" AimBionicNamespaceStatus
aim_bionic_namespace_teardown(AimBionicNamespace *instance) {
  if (instance == nullptr)
    return AIM_BIONIC_NAMESPACE_INVALID_ARGUMENT;
  std::array<AimBionicProviderBinding, AIM_BIONIC_PROVIDER_COUNT>
      bindings{};
  {
    std::unique_lock<std::mutex> lock(instance->mutex);
    if (instance->state == State::kDead) {
      return AIM_BIONIC_NAMESPACE_OK;
    }
    if (instance->state == State::kDraining) {
      instance->idle.wait(
          lock, [instance] { return instance->state == State::kDead; });
      return AIM_BIONIC_NAMESPACE_OK;
    }
    instance->state = State::kDraining;
    instance->idle.wait(lock, [instance] { return instance->in_flight == 0; });
    bindings = instance->bindings;
  }

  for (AimBionicProviderId provider : kReleaseOrder) {
    const AimBionicProviderBinding &binding =
        bindings[static_cast<size_t>(provider)];
    if (binding.release != nullptr)
      binding.release(binding.context);
  }
  {
    std::lock_guard<std::mutex> lock(instance->mutex);
    instance->state = State::kDead;
    instance->idle.notify_all();
  }
  return AIM_BIONIC_NAMESPACE_OK;
}

extern "C" void
aim_bionic_namespace_destroy(AimBionicNamespace *instance) {
  if (instance == nullptr)
    return;
  (void)aim_bionic_namespace_teardown(instance);
  delete instance;
}

extern "C" size_t aim_bionic_namespace_owned_count() {
  return sizeof(kOwnership) / sizeof(kOwnership[0]);
}

extern "C" size_t aim_bionic_namespace_unsupported_libc_count() {
  return kUnsupportedLibcCount;
}

extern "C" int aim_bionic_namespace_unsupported_libc(
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
aim_bionic_provider_name(AimBionicProviderId provider) {
  return provider >= 0 && provider < AIM_BIONIC_PROVIDER_COUNT
             ? kProviderNames[static_cast<size_t>(provider)]
             : nullptr;
}

extern "C" const char *
aim_bionic_namespace_status_name(AimBionicNamespaceStatus status) {
  switch (status) {
  case AIM_BIONIC_NAMESPACE_OK:
    return "ok";
  case AIM_BIONIC_NAMESPACE_INVALID_ARGUMENT:
    return "invalid-argument";
  case AIM_BIONIC_NAMESPACE_DUPLICATE_BINDING:
    return "duplicate-binding";
  case AIM_BIONIC_NAMESPACE_PROVIDER_UNBOUND:
    return "provider-unbound";
  case AIM_BIONIC_NAMESPACE_NOT_SEALED:
    return "not-sealed";
  case AIM_BIONIC_NAMESPACE_ALREADY_SEALED:
    return "already-sealed";
  case AIM_BIONIC_NAMESPACE_SHUTTING_DOWN:
    return "shutting-down";
  case AIM_BIONIC_NAMESPACE_UNKNOWN_SONAME:
    return "unknown-soname";
  case AIM_BIONIC_NAMESPACE_UNKNOWN_VERSION:
    return "unknown-version";
  case AIM_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL:
    return "unsupported-symbol";
  case AIM_BIONIC_NAMESPACE_PROVIDER_REJECTED:
    return "provider-rejected";
  }
  return "invalid-status";
}
