# Bionic runtime provider closure

This module is the concrete link closure behind the generated closed Bionic
namespace. It does not add another implementation of a libc symbol. Instead,
it combines the existing C, C++, and Rust provider archives so
`aim_bionic_namespace_bind_builtins` can bind every manifest owner to
its actual resolver.

The current closure contains thirty-two providers and resolves 185 exact
routes: all 160 pinned Android 35 arm64 `libc++_shared.so` libc-family imports,
seven reviewed libc extensions for pthread creation, central close dispatch,
socket I/O, and bounded DNS, plus all 18 public `liblog.so` exports. Unknown
SONAMEs, symbols, or GNU versions never fall back to dyld or host `dlsym`.

Run:

```sh
tools/build-bionic-runtime-provider-closure.sh
```

The gate builds one Rust static archive for the stateful filesystem,
process-state, stdio, and DSO-lifecycle owners; one native archive for the
remaining providers (including the central descriptor broker, socket/DNS,
Bionic abort/message state, and the provider-only
wide-float, wide-stdio, scanf, swprintf, ioctl, strftime, and sendfile owners) and
namespace; and the pinned AOSP gdtoa float-conversion
archive. The wide-float owner reuses the one allocator, gdtoa/errno, and ICU
closure already present; it embeds none of those providers. The gate then links a real arm64
executable with Android ICU 76.1 and AOSP liblog, seals the namespace, and
resolves every generated route through the actual provider callbacks. Host or
dynamic ICU linkage is rejected.

The wide-stdio owner contributes only its C++ provider and C entry shims to the
native archive. Its locale and ICU 76.1 edges remain unresolved until the
closure's existing locale object and single static ICU foundation satisfy
them. The Rust provider archive must define no wide-stdio or ICU symbol. The
full-link gate audits every ICU 76/init definition across all closure archives
for duplicates, then exercises `ungetwc`/`getwc` through the central stdio
process lease before namespace sealing.

The syslog owner reuses the closure's single formatter, errno, and AOSP liblog
owners; it does not embed copies. The full-link smoke activates an immutable
guest process tag and verifies that null-tag logging retains the owned copy.
The ART namespace intentionally does not invent a tag: until ART supplies a
real guest process/package tag, a guest `syslog` call without a preceding
`openlog` is the facade's explicit `ENOTSUP` capability boundary. Pinned
libc++ always calls `openlog("libc++", ...)` before its owned syslog call.

The formatted-stdio owner contributes only `fprintf`, `vfprintf`, and their
closed resolver. It composes the existing bounded Android `va_list` formatter
with the existing provider-local Android `FILE`/`fwrite` core; the link audit
requires exactly one format, stdio, allocator, and errno definition.

The binary128 conversion archive owns `strtold`, `strtold_l`, and `wcstold`
through Android AAPCS64 q0-return entries. It is linked before the
allocator/native archive, then the existing gdtoa/errno float archive, then
static ICU. It embeds no duplicate allocator, common gdtoa, errno, or ICU
owner.

The ioctl owner accepts only the Android `RNDGETENTCNT` request and obtains FD
kind through the process filesystem owner. The strftime owner is activated
with an immutable fixed-offset UTC timezone until a product timezone owner is
available. Both drain in-flight calls after guest finalizers. The swprintf
owner reuses the existing formatter, errno, allocator, and gdtoa providers.
The sendfile owner delegates only to the process filesystem callback and
drains before that filesystem owner is uninstalled.

The network owner publishes socket objects only through the central descriptor
broker. The shared `close@LIBC` route dispatches broker-shaped tokens to that
broker and all other tokens to the filesystem owner; the filesystem allocator
reserves the broker marker range, so stale tokens cannot alias across owners.
Namespace admission drains before DNS result retirement, socket teardown, and
the final filesystem uninstall.

The syscall owner contributes the one variadic `syscall@LIBC` entry and its
exact libc++ gettid/futex/libunwind-probe dispatcher. It reuses the closure's
Bionic errno owner and never forwards Linux syscall numbers to Darwin.

The embedding lifetime is strict: create, bind all providers, seal, load and
run guest DSOs, run guest static and `__cxa_finalize` teardown, unload the ELF
graph, then quiescently tear down the namespace. The closure currently keeps
the filesystem and stdio virtual descriptor owners separate; sharing one
Android open-file-description table is a later compatibility boundary.
