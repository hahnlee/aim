# FD ownership tags

This Rust module owns process-local, per-descriptor tags used by the production
socket/FD facade. Sparse storage supports generation-encoded guest descriptors;
tags do not belong to shared open-file descriptions. An atomic logical exchange
under one mutex leaves the actual tag unchanged on mismatch, and removes a tag
on a successful exchange to zero. Diagnostics and actual close run after unlock.

The native `bionic-socket-broker-adapter/src/fdsan.cc` is the ABI boundary. Both
ordinary close and tagged close reach it before dispatch to central, filesystem
or shared-memory owners. The previous exchange no-op and unchecked tagged close
have been removed. Its current error policy is Android libc's initial fatal
policy unless the property-aware setter or linker target-SDK policy changes it.

`darwin_art_fdsan_get_owner_tag` reads the same store under its existing lock.
The socket facade exports it as `android_fdsan_get_owner_tag@LIBC_Q`; the sealed
provider ownership manifest also registers that exact route. Missing/negative
descriptors return zero, queries do not allocate entries, and close clears the
tag before descriptor reuse. This follows pinned Android16 Bionic fdsan.cpp's
GetFdEntry/close_tag query, not libcutils' non-Bionic host stub.

The host process boundary registers pthread fork handlers during FD provider
activation, before guest callbacks/threads. Prepare quiesces mutations; parent
and child each unlock their own copy, preserving inherited tags. Tests fork while
another writer holds the lock, check independent parent/child values, and exercise
subsequently registered callbacks accessing tags in all three fork phases.

Initialization must happen at process startup; concurrent fork during first
initialization is not supported. Direct syscall fork and vfork do not use this
pthread contract. Remaining requirements include vfork handling and descriptor
replacement/internal-close audit. The linker SDK component calls the real
property-aware severity setter; production loader cutover remains separate. This is
FDSAN-specific fork safety, not safety of all runtime locks or the entire APK.
