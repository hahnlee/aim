# Bionic socket central-broker adapter

This new-only process owner consumes the public central FD broker ABI v3. Its
first production slice routes Android `socket`, `connect`, `send`, `recv`, and
generic `close` through one generation-tagged descriptor namespace. Socket
callbacks never expose a Darwin descriptor or broker object key to guest code.
Only broker-marker tokens enter the central close path; all other descriptors
delegate directly to the filesystem owner even while networking is inactive or
draining. The filesystem allocator reserves the marker range.

The standalone gate executes the existing real Android AArch64 HTTP fixture
against a Darwin numeric-loopback listener, with the existing bounded DNS and
thread-local errno providers. It verifies exact `libc.so@LIBC` resolution,
partial I/O, generic broker close, DNS retirement, host errno preservation, and
quiescent owner uninstall under ASan+UBSan and TSan. DNS resolution and free
are lifecycle wrappers: both hold process admission, and a live result blocks
deactivation until it is retired. Deactivation drains calls before DNS reset
and broker destruction.

This is intentionally the minimum actual-ART HTTP slice. The broker v3 request
ABI already provides typed operations for bind/listen/accept4/shutdown,
sendto/recvfrom, socket options, and peer/local names; exposing those symbols
requires adding the corresponding translated wrappers without changing token
ownership.
