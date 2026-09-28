# Bionic network runtime integration freeze

This gate freezes and executes the smallest safe path from a real ART native
load to Darwin loopback networking. The shared runtime binds a process-scoped
socket owner through the central descriptor broker and routes bounded DNS
through the same admission lifetime.

The Android NDK r28c/API-35 AArch64 fixture exports `JNI_OnLoad`, obtains the
live ART `JNIEnv`, and registers one static `(I)I` native method. The method
performs a numeric-loopback HTTP/1.0 request through these exact imports:

```text
__errno -> getaddrinfo -> socket -> connect -> send/recv -> close
        -> freeaddrinfo
```

It accepts only a Java-supplied TCP port and hard-codes `127.0.0.1`; it cannot
request external DNS or connect to a non-loopback address. `JNI_OnLoad` itself
does no I/O. This allows the host probe to create the listener before ART loads
the DSO and to execute the request only after native registration succeeds.

`manifests/routes.tsv` is the process namespace contract. Every symbol
is owned once under exact `libc.so`/`LIBC`; notably, public `close` belongs to
the central descriptor broker rather than the filesystem or socket provider.
`manifests/lifecycle.tsv` fixes admission and teardown order. Guest DSO
finalizers run while providers are still callable; namespace admission then
closes, in-flight operations drain, DNS quarantine is reclaimed, the socket
owner is uninstalled, and the empty broker is destroyed.
`manifests/required-broker-abi.tsv` freezes the broker v3 lease primitive, while
`manifests/shared-scope.tsv` records the shared surfaces landed atomically.

## Landed runtime path

Broker ABI v3 holds exact owner/kind/generation leases for typed socket
operations and atomically publishes or rolls back `accept4` children. The
socket adapter owns no private guest token table. Its central `close` route
sends only `0x40000000`-marked tokens to the broker and delegates all other
tokens to the filesystem owner; the filesystem allocator reserves that marker
range. Stale central tokens therefore cannot alias filesystem descriptors.

DNS entrypoints are adapter wrappers, not raw DNS resolver addresses. Each
call holds the socket process admission lease, and a live result prevents
deactivation until `freeaddrinfo` retires it. Last-owner teardown drains route
admission, reclaims DNS, uninstalls the socket owner, destroys the empty broker,
then releases filesystem authority.

`art-bootstrap probe-runtime-network` builds an isolated baseline-plus-network
DEX and a real API-35 AArch64 DSO, copies the sole DSO into a private
directory (`0500`, file `0400`), and loads it through ART `JavaVMExt`. A bounded
Darwin listener accepts the exact HTTP request on `127.0.0.1`; the registered
`(I)I` JNI method must return 42. Shutdown requires zero JNI trampolines,
socket objects, DNS results, and network owners. No external DNS or Internet
route is admitted.

Run the structural preflight with:

```sh
./tools/bionic-network-runtime-integration/audit.sh
```

The standalone gate compiles the real Android JNI DSO, checks its sole export
and exact eight-symbol import surface, verifies broker v3 and the lifecycle
adapter, and checks the pinned route/lifecycle freeze. Run the actual ART gate
with `cargo run --manifest-path crates/art-bootstrap/Cargo.toml --
probe-runtime-network`.
