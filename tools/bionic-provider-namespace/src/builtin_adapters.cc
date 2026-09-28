#include "aim_bionic_builtin_adapters.h"

namespace {

using SymbolFunction = void (*)(void);

extern "C" SymbolFunction aim_bionic_libc_leaf_resolve(const char *);
extern "C" SymbolFunction aim_bionic_allocator_resolve(const char *);
extern "C" SymbolFunction aim_bionic_errno_resolve(const char *);
extern "C" SymbolFunction aim_bionic_fs_resolve(const char *);
extern "C" SymbolFunction aim_bionic_ftw_resolve(const char *);
extern "C" SymbolFunction aim_bionic_time_resolve(const char *);
extern "C" uintptr_t aim_bionic_time_data_resolve(const char *);
extern "C" void *aim_bionic_pthread_resolve(const char *, const char *,
                                                   const char *);
extern "C" SymbolFunction aim_bionic_process_state_resolve(const char *);
// init's property service client (aim-runtime property_ffi): the
// setter every process's __system_property_set resolves to.
extern "C" int aim_bionic_property_service_set(const char *, const char *);
extern "C" uintptr_t aim_bionic_process_state_data_resolve(const char *);
extern "C" void *aim_dl_phdr_resolve(const char *, const char *,
                                            const char *);
extern "C" SymbolFunction aim_bionic_stdio_resolve(const char *);
extern "C" void *
aim_bionic_wide_stdio_resolve(const char *, const char *, const char *);
extern "C" void *aim_bionic_scanf_resolve(const char *, const char *,
                                                 const char *);
extern "C" void *aim_bionic_swprintf_resolve(const char *, const char *,
                                                    const char *);
extern "C" void *aim_bionic_ioctl_resolve(const char *, const char *,
                                                 const char *);
extern "C" void *aim_bionic_strftime_resolve(const char *, const char *,
                                                    const char *);
extern "C" void *aim_bionic_sendfile_resolve(const char *, const char *,
                                                    const char *);
extern "C" void *aim_bionic_socket_broker_resolve(const char *,
                                                         const char *,
                                                         const char *);
extern "C" uintptr_t aim_bionic_socket_broker_data_resolve(
    const char *, const char *, const char *);
extern "C" void *aim_bionic_socket_broker_dns_resolve(const char *,
                                                             const char *,
                                                             const char *);
extern "C" void *aim_bionic_math_resolve(const char *, const char *,
                                                 const char *);
extern "C" SymbolFunction aim_bionic_vm_resolve(const char *);
extern "C" void *aim_bionic_locale_resolve(const char *, const char *,
                                                  const char *);
extern "C" SymbolFunction aim_bionic_numeric_resolve(const char *);
extern "C" void *aim_bionic_float_conversion_resolve(const char *,
                                                            const char *,
                                                            const char *);
extern "C" SymbolFunction aim_bionic_format_resolve(const char *);
extern "C" SymbolFunction aim_bionic_strerror_resolve(const char *);
extern "C" void *aim_bionic_wide_integer_resolve(const char *,
                                                        const char *,
                                                        const char *);
extern "C" void *
aim_bionic_wide_float_resolve(const char *, const char *, const char *);
extern "C" void *aim_bionic_binary128_conversion_resolve(const char *,
                                                                const char *,
                                                                const char *);
extern "C" void *aim_bionic_abort_resolve(const char *, const char *,
                                                 const char *);
extern "C" void *aim_android_binder_ndk_resolve(const char *,
                                                        const char *,
                                                        const char *);
extern "C" void *aim_android_aaudio_resolve(const char *, const char *,
                                                    const char *);
extern "C" uintptr_t aim_liblog_provider_resolve(const char *,
                                                        const char *);
extern "C" SymbolFunction aim_bionic_dso_lifecycle_resolve(const char *);
extern "C" SymbolFunction
aim_bionic_syslog_resolve(const char *, const char *, const char *);
extern "C" SymbolFunction
aim_bionic_formatted_stdio_resolve(const char *, const char *,
                                          const char *);
extern "C" SymbolFunction
aim_bionic_syscall_resolve(const char *, const char *, const char *);

uintptr_t Address(SymbolFunction function) {
  return reinterpret_cast<uintptr_t>(function);
}

uintptr_t Leaf(void *, const char *, const char *symbol, const char *) {
  return Address(aim_bionic_libc_leaf_resolve(symbol));
}
uintptr_t Allocator(void *, const char *, const char *symbol, const char *) {
  return Address(aim_bionic_allocator_resolve(symbol));
}
uintptr_t Errno(void *, const char *, const char *symbol, const char *) {
  return Address(aim_bionic_errno_resolve(symbol));
}
uintptr_t Filesystem(void *, const char *, const char *symbol, const char *) {
  const auto traversal = aim_bionic_ftw_resolve(symbol);
  return Address(traversal != nullptr ? traversal
                                     : aim_bionic_fs_resolve(symbol));
}
uintptr_t Time(void *, const char *, const char *symbol, const char *) {
  const uintptr_t data = aim_bionic_time_data_resolve(symbol);
  if (data != 0) return data;
  return Address(aim_bionic_time_resolve(symbol));
}
uintptr_t Pthread(void *, const char *soname, const char *symbol,
                  const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_bionic_pthread_resolve(soname, symbol, version));
}
// Adapters never import host libc: compare symbol names locally.
bool SameSymbol(const char *symbol, const char *name) {
  if (symbol == nullptr) return false;
  while (*symbol != '\0' && *symbol == *name) {
    ++symbol;
    ++name;
  }
  return *symbol == *name;
}
uintptr_t ProcessState(void *, const char *, const char *symbol, const char *) {
  if (SameSymbol(symbol, "__system_property_set"))
    return reinterpret_cast<uintptr_t>(&aim_bionic_property_service_set);
  const uintptr_t data = aim_bionic_process_state_data_resolve(symbol);
  if (data != 0) return data;
  return Address(aim_bionic_process_state_resolve(symbol));
}
uintptr_t Phdr(void *, const char *soname, const char *symbol,
               const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_dl_phdr_resolve(soname, symbol, version));
}
uintptr_t Stdio(void *, const char *, const char *symbol, const char *) {
  return Address(aim_bionic_stdio_resolve(symbol));
}
uintptr_t WideStdio(void *, const char *soname, const char *symbol,
                    const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_bionic_wide_stdio_resolve(soname, symbol, version));
}
uintptr_t Scanf(void *, const char *soname, const char *symbol,
                const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_bionic_scanf_resolve(soname, symbol, version));
}
uintptr_t Swprintf(void *, const char *soname, const char *symbol,
                   const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_bionic_swprintf_resolve(soname, symbol, version));
}
uintptr_t Ioctl(void *, const char *soname, const char *symbol,
                const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_bionic_ioctl_resolve(soname, symbol, version));
}
uintptr_t Strftime(void *, const char *soname, const char *symbol,
                   const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_bionic_strftime_resolve(soname, symbol, version));
}
uintptr_t Sendfile(void *, const char *soname, const char *symbol,
                   const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_bionic_sendfile_resolve(soname, symbol, version));
}
uintptr_t CentralFdBroker(void *, const char *soname, const char *symbol,
                          const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_bionic_socket_broker_resolve(soname, symbol, version));
}
uintptr_t Socket(void *, const char *soname, const char *symbol,
                 const char *version) {
  const uintptr_t data =
      aim_bionic_socket_broker_data_resolve(soname, symbol, version);
  if (data != 0) return data;
  return reinterpret_cast<uintptr_t>(
      aim_bionic_socket_broker_resolve(soname, symbol, version));
}
uintptr_t Dns(void *, const char *soname, const char *symbol,
              const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_bionic_socket_broker_dns_resolve(soname, symbol, version));
}
uintptr_t Math(void *, const char *soname, const char *symbol,
               const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_bionic_math_resolve(soname, symbol, version));
}
uintptr_t Vm(void *, const char *, const char *symbol, const char *) {
  return Address(aim_bionic_vm_resolve(symbol));
}
uintptr_t BinderNdk(void *, const char *soname, const char *symbol,
                    const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_android_binder_ndk_resolve(soname, symbol, version));
}
uintptr_t AAudio(void *, const char *soname, const char *symbol,
                 const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_android_aaudio_resolve(soname, symbol, version));
}
uintptr_t Locale(void *, const char *soname, const char *symbol,
                 const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_bionic_locale_resolve(soname, symbol, version));
}
uintptr_t Numeric(void *, const char *, const char *symbol, const char *) {
  return Address(aim_bionic_numeric_resolve(symbol));
}
uintptr_t FloatConversion(void *, const char *soname, const char *symbol,
                          const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_bionic_float_conversion_resolve(soname, symbol, version));
}
uintptr_t Format(void *, const char *, const char *symbol, const char *) {
  return Address(aim_bionic_format_resolve(symbol));
}
uintptr_t Strerror(void *, const char *, const char *symbol, const char *) {
  return Address(aim_bionic_strerror_resolve(symbol));
}
uintptr_t WideInteger(void *, const char *soname, const char *symbol,
                      const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_bionic_wide_integer_resolve(soname, symbol, version));
}
uintptr_t WideFloat(void *, const char *soname, const char *symbol,
                    const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_bionic_wide_float_resolve(soname, symbol, version));
}
uintptr_t Binary128Conversion(void *, const char *soname, const char *symbol,
                              const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_bionic_binary128_conversion_resolve(soname, symbol, version));
}
uintptr_t Abort(void *, const char *soname, const char *symbol,
                const char *version) {
  return reinterpret_cast<uintptr_t>(
      aim_bionic_abort_resolve(soname, symbol, version));
}
uintptr_t Liblog(void *, const char *, const char *symbol,
                 const char *version) {
  return aim_liblog_provider_resolve(symbol, version);
}
uintptr_t DsoLifecycle(void *, const char *, const char *symbol, const char *) {
  return Address(aim_bionic_dso_lifecycle_resolve(symbol));
}
uintptr_t Syslog(void *, const char *soname, const char *symbol,
                 const char *version) {
  return Address(aim_bionic_syslog_resolve(soname, symbol, version));
}
uintptr_t FormattedStdio(void *, const char *soname, const char *symbol,
                         const char *version) {
  return Address(
      aim_bionic_formatted_stdio_resolve(soname, symbol, version));
}
uintptr_t Syscall(void *, const char *soname, const char *symbol,
                  const char *version) {
  return Address(aim_bionic_syscall_resolve(soname, symbol, version));
}

constexpr AimBionicProviderResolve kResolvers[] = {
    Leaf,
    Allocator,
    Errno,
    Filesystem,
    Time,
    Pthread,
    ProcessState,
    Phdr,
    Stdio,
    Locale,
    Numeric,
    FloatConversion,
    Format,
    Strerror,
    WideInteger,
    Abort,
    Liblog,
    DsoLifecycle,
    WideFloat,
    Syslog,
    FormattedStdio,
    Syscall,
    Binary128Conversion,
    WideStdio,
    Scanf,
    Swprintf,
    Ioctl,
    Strftime,
    Sendfile,
    CentralFdBroker,
    Socket,
    Dns,
    Math,
    Vm,
    BinderNdk,
    AAudio,
};
static_assert(sizeof(kResolvers) / sizeof(kResolvers[0]) ==
              AIM_BIONIC_PROVIDER_COUNT);

} // namespace

extern "C" AimBionicNamespaceStatus
aim_bionic_namespace_bind_builtins(
    AimBionicNamespace *instance,
    const AimBionicProviderReleaseHooks *release_hooks) {
  if (instance == nullptr)
    return AIM_BIONIC_NAMESPACE_INVALID_ARGUMENT;
  for (int index = 0; index < AIM_BIONIC_PROVIDER_COUNT; ++index) {
    const auto provider = static_cast<AimBionicProviderId>(index);
    AimBionicProviderBinding binding{
        provider,
        release_hooks == nullptr ? nullptr : release_hooks->context[index],
        kResolvers[index],
        release_hooks == nullptr ? nullptr : release_hooks->release[index],
    };
    const AimBionicNamespaceStatus status =
        aim_bionic_namespace_bind(instance, &binding);
    if (status != AIM_BIONIC_NAMESPACE_OK)
      return status;
  }
  return AIM_BIONIC_NAMESPACE_OK;
}
