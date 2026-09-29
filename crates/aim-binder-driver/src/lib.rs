//! The Linux binder driver for original Android ELF userspace (ADR 0012).
//!
//! The original `libbinder` talks to `/dev/binder`, `/dev/hwbinder` and
//! `/dev/vndbinder` with ioctls and a read-only mmap'ed receive buffer. This
//! crate is that driver's core, written against
//! `include/uapi/linux/android/binder.h` and `drivers/android/binder.c` of
//! the Android common kernel, with no dependency on the syscall layer:
//!
//! - [`Driver::open`] / [`Driver::release`] are the file's open and last
//!   close; [`Driver::mmap`] installs the receive buffer;
//! - [`Driver::ioctl`] takes the ioctl number and its argument bytes, and
//!   reaches guest memory and the guest fd table through [`GuestProcess`];
//! - [`Driver::poll`] and [`Driver::set_notifier`] back epoll on the fd.
//!
//! State is split as in Linux: global (contexts and their context managers,
//! nodes, transactions, death notifications), per process (threads, node
//! and handle tables, the process work queue, the buffer allocator) and per
//! thread (looper state, work queue, transaction stack). Credentials come
//! from the process registry's Android identity ([`Credentials`]).
//!
//! Where it runs: the core is location-independent. Inside one address
//! space it is used directly (the integration-test harness in
//! `tests/support` runs several "processes" this way). Across host processes it is meant to live in
//! aimd, with each guest's syscall layer forwarding ioctls; see
//! `docs/binder-driver.md` for the transport and buffer design.
//!
//! Freezing is not supported: `BINDER_FREEZE` and `BINDER_GET_FROZEN_INFO`
//! fail with `EINVAL`, as on a kernel without the cgroup freezer.

mod alloc;
mod driver;
mod host;
mod read;
mod release;
mod state;
mod trace;
mod transaction;
pub mod uapi;
mod write;

pub use driver::{Device, Driver, MAX_MAPPING, ProcHandle};
pub use host::{Credentials, Errno, File, GuestProcess, HeapReceiveMemory, ReceiveMemory, errno};
pub use state::{Notifier, Tid};
pub use trace::TraceRecord;
