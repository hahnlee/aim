# Bionic DNS facade

This standalone provider owns the minimum coherent Bionic name/address cluster:
`getaddrinfo`, `freeaddrinfo`, `gai_strerror`, `getnameinfo`, `inet_ntop`,
`gethostbyname`, and `getservbyname`
under the exact `libc.so`/`LIBC` namespace. A real NDK r28c/API-35 Android
AArch64 ELF exercises these imports through the repository ELF loader. Resolution is closed; no
dyld, `dlsym`, or alternate SONAME path exists.

## Resolver policy

Forward lookup uses Darwin's configured system resolver for explicit DNS-style
hostnames, which is the compatibility-layer equivalent of Android's
netd-configured resolver. The facade validates the name and appends a trailing
dot before the host call, so an APK cannot accidentally inherit search-domain
expansion. Unqualified names and `.local` remain closed to avoid leaking host
LAN/mDNS policy; null passive nodes, case-insensitive `localhost`, and textual
numeric IPv4/IPv6 remain supported. For `getaddrinfo`, services must be null
or decimal ports from 0 through 65535, so `/etc/services` is never inherited.
Set
`AIM_DEBUG_DNS=1` to log requested and absolute query names plus the
host resolver status without changing policy.

The legacy host and service entry points use real Darwin resolver results but
copy them into bounded Android-shaped, thread-local `hostent` and `servent`
storage. Host names use the same explicit numeric/localhost/FQDN policy as
`getaddrinfo`; service names are looked up through the host service database,
with failures reflected in the Android `h_errno` cell.

Supported Android hints are `AI_PASSIVE`, `AI_CANONNAME`, `AI_NUMERICHOST`,
`AI_NUMERICSERV`, and `AI_ADDRCONFIG`; family is `AF_UNSPEC`, `AF_INET`, or Android `AF_INET6=10`,
with stream/datagram and TCP/UDP protocols. Unknown flags and nonempty result
fields in hints fail closed. V4-mapped synthesis, IDN, DNS
search, and arbitrary reverse lookup remain explicit boundaries in
`manifests/unsupported.tsv`.

`getnameinfo` accepts Android IPv4/IPv6 sockaddr layouts and requires numeric
host/service flags for whichever outputs are requested. It translates the
address to Darwin and returns numeric reverse text only. `NI_DGRAM` is mapped;
name-requiring flags and a request without `NI_NUMERICHOST` return a Bionic EAI
failure instead of entering host PTR or mDNS policy.

`inet_ntop` translates Android `AF_INET6=10` to Darwin `AF_INET6=30` before
using the BSD numeric formatter; address bytes and caller-buffer ownership are
unchanged. Host failures are translated into the Bionic errno cell.

## ABI and ownership

The Android arm64 `addrinfo` is copied field by field even though its 48-byte
top-level layout currently matches Darwin. Family, flags, socket type,
protocol, address length, canonical name, and linked-list ownership are not
borrowed. Each Darwin result is filtered to the supported family/type/protocol,
then deep-copied into an Android `addrinfo` node and a 16-byte `sockaddr_in` or
28-byte `sockaddr_in6` without Darwin's length/family bytes. A single result is
capped at 64 nodes and the standalone owner tracks at most 256 result heads.

`freeaddrinfo` validates ownership by head identity and atomically marks a
result retired. It does not dereference foreign pointers and does not reclaim
node storage immediately. This quarantine makes a concurrent already-started
reader and one or more frees memory-safe and TSan-clean; the list is logically
invalid after the first free but remains immutable. Quiescent lifecycle reset
reclaims live and retired lists. Reset must run only after all guest readers
have stopped. This bounded ownership policy avoids both Darwin allocator
exposure and unsound immediate-free races; runtime integration should bind the
same reset to guest teardown.

Darwin EAI results are explicitly mapped to Android's pinned BSD-derived EAI
numbers, and `gai_strerror` returns provider-owned static Android messages.
`EAI_SYSTEM` stores a translated Bionic errno. Every entry point restores host
`errno`; successful and ordinary EAI returns do not leak Darwin errno state.

## Gate

`audit.sh` SHA-pins the exact NDK netdb/socket headers plus AOSP Bionic
`getaddrinfo`, `getnameinfo`, netdb, and public ABI sources. It statically
asserts Android/Darwin constants, sizes, and offsets, verifies that the actual
fixture has exactly five `@LIBC` imports, and rejects dynamic lookup. Runtime
tests cover localhost, passive wildcard, numeric IPv4/IPv6, numeric reverse,
external-name validation, closed search/mDNS/name-service policy, result ownership, duplicate concurrent
free with an active reader, host errno preservation, and quiescent reclamation
under ASan+UBSan and TSan. Build output remains under temporary or ignored
`_build` paths; the source module stays target-clean.
