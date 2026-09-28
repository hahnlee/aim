# Bionic ioctl facade

This standalone provider owns the one `ioctl@LIBC` import in the SHA-locked
NDK r28c/API-35 arm64 `libc++_shared.so`. The real ELF has exactly one call at
`0xdc74c`: `std::__ndk1::random_device::entropy() const` passes its open file
descriptor, Android `RNDGETENTCNT` (`0x80045200`), and an `int*`. The pinned
LLVM source returns zero entropy on ioctl failure and otherwise clamps the
kernel-reported count to the 32 result bits.

Android and Darwin variadic ABIs are not assumed interchangeable. The AArch64
entry captures x0-x7 before a non-variadic core decodes the exact observed
three-argument form. No Darwin `ioctl`, raw request forwarding, dyld, or
`dlsym` path exists.

The provider does not accept host descriptors. Before guest execution, the
embedding filesystem/virtual-FD owner activates an immutable callback that
classifies its own token as a random device, another known kind, an invalid
descriptor, or a capability failure. This keeps descriptor identity with the
FD owner while Android request and exact random-device policy remain here.
Deactivation stops admission and waits for in-flight callbacks before the
callback context may be destroyed.

Only `RNDGETENTCNT` on a random-device token is supported, and it reports 32
bits, the exact width consumed by this pinned libc++ callsite. Invalid tokens
fail with Bionic `EBADF`; other known kinds or requests fail with `ENOTTY`; a
missing/unavailable FD provider fails with `ENOSYS`; an invalid output range
fails with `EFAULT`. Null, unaligned, unmapped, or non-readable/writable output is
rejected; the final store uses `mach_vm_write`, avoiding an unchecked direct
guest-pointer dereference if mappings race. The VM check and Mach write are
separate kernel operations, so an embedding must still keep guest mappings
stable during the call to prevent a concurrent remap from redirecting the
write. Host errno is preserved, and success does not clear the
guest's existing Bionic errno. Unknown requests never reach Darwin.

`audit.sh` pins the NDK ELF and UAPI headers, the Bionic variadic wrapper, the
LLVM random-device source, the exact disassembly callsite, and all manifests.
It builds a real Android AArch64 ELF importing only `ioctl@LIBC`, then verifies
the resolver, ABI capture, FD-kind routing, errno behavior, writable-memory
checks, concurrent quiescent deactivation, and fail-closed boundary under
ASan/UBSan.

The current filesystem provider does not retain random-device kind in its
virtual-FD table. Extending that metadata and binding this callback is a
separate integration task; this standalone provider does not guess from token
numbers or unwrap host descriptors.
