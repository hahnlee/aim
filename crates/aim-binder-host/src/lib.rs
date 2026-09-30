//! The binder driver across host processes (ADR 0012, decision 7; #168).
//!
//! `aim-binder-driver` holds all binder state: nodes and references span
//! processes, and one process's transactions write another's work queues and
//! receive buffer. This crate puts one driver in a daemon and connects each
//! guest's syscall layer to it:
//!
//! - [`server`]: the daemon side, embeddable in any host process
//!   (`aim-binderd` runs it alone; aimd is meant to host it);
//! - [`client`]: what the syscall layer calls for `open`, `ioctl`, `mmap`
//!   and poll registration on a binder fd;
//! - [`wire`] and [`mach`]: the messages and the Mach primitives under them;
//! - [`local`], [`parcel`] and [`appops`]: a binder process on the host
//!   itself, for native system services (ADR 0013).
//!
//! Each guest ioctl is one `mach_msg(SEND|RCV)` from the calling thread to
//! its own daemon thread, which runs the driver's ioctl and may block in it.
//! Transactions are copied into the target's receive buffer once: it is
//! shared memory, mapped read-write in the daemon and read-only in the
//! guest. Fds travel as fileports. See `docs/binder-driver.md`.

pub mod appops;
pub mod client;
pub mod local;
pub mod mach;
pub mod parcel;
pub mod server;
pub mod wire;
