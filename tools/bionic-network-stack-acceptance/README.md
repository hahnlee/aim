# Bionic network stack acceptance

This new-only acceptance gate composes the frozen standalone Bionic socket,
DNS, and errno providers without changing their namespaces or ownership. Its
real NDK r28c/API-35 Android AArch64 ELF imports exactly eight `libc.so`/`LIBC`
symbols and executes this client path:

```text
getaddrinfo -> socket -> connect -> send/recv -> close -> freeaddrinfo
```

The host side creates only Darwin IPv4/IPv6 loopback listeners. It verifies the
accepted peer is loopback, reads an HTTP/1.0 `GET /acceptance`, returns a fixed
`200 OK` response with body `HELLO`, shuts down the connection, closes the
listener, and joins the server thread. No test performs external DNS or an
Internet connection.

The Android client limits every `send` and `recv` call to at most three bytes,
loops until the complete request is written and EOF closes the response, and
retries `connect`/`send`/`recv` only when its Bionic `__errno` cell reports
`EINTR`. One server deliberately withholds its response after reading the
request. The runner waits until the socket provider has admitted the blocking
`recv`, delivers `SIGUSR1` to that exact client pthread, and releases the
response after the interrupted call retries. This makes the EINTR path and
short-I/O loops actual Android ELF behavior rather than host-only simulation.

Coverage includes numeric IPv4, numeric IPv6, and `localhost` with
`AF_UNSPEC`. The localhost test binds only IPv4, so a Darwin result ordering
that presents `::1` first exercises failed-address close and next-address
fallback before the successful request. Four Android client threads then use
the same IPv4 listener concurrently. Each thread retains host `errno`, each DNS
result is retired via the DNS provider, and every virtual socket token is
closed. Only after clients and servers quiesce does the runner reclaim DNS
quarantine and reset the socket owner.

`manifests/boundary.tsv` records the deliberately narrow scope: HTTP/1.0 only,
no TLS, redirects, external resolver policy, or Internet. The combined
resolver accepts only the eight manifest symbols with exact SONAME/version and
has no dyld fallback. The audit pins the committed dependency trees, NDK,
fixture, runner, manifests, and provider source inputs; checks exact ELF
imports; then runs the complete IPv4/IPv6/EINTR/concurrency/teardown story under
ASan+UBSan and TSan. All cargo and object output is temporary, leaving the
source tree target-clean.
