# Bionic sendfile facade

This provider owns the one `sendfile@LIBC` import in the pinned API 35 arm64
`libc++_shared.so`. Its exact owner is `filesystem::copy_file_impl`, which calls
`sendfile(output_fd, input_fd, nullptr, remaining_size)` until the source size
is exhausted.

The facade never calls Darwin `sendfile`, a raw syscall, or a host descriptor.
It leases a process-wide callback into the central virtual-fd owner. That owner
must perform one atomic transfer and implement Linux offset rules. Successful
partial transfers and EOF are returned directly; callback failures publish
Android errno through the coherent Bionic TLS provider. A non-null `off_t*` is
read and written only after a Mach VM range check, leaves the input descriptor
position unchanged, and advances by exactly the returned byte count.

`audit.sh` pins the LLVM source, NDK libraries and headers, exact libc++ import
and callsite, builds an Android AArch64 ELF fixture, and executes it through the
real loader. The callback fixture covers partial copy, EOF, explicit offset,
bad descriptors, and an owner-published `EROFS`. ASan/UBSan cover the provider
and loaded fixture; TSan covers its activation/dispatch/drain synchronization.
