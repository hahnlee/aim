//! aimd's init role (ADR 0012 item 6): boots the original Android
//! userspace of a derived image on the Linux syscall layer.
//!
//! `aim-android-init` models init's inputs and its action engine; this
//! crate performs what init does with them on a Darwin host:
//!
//! - [`props`], [`propsvc`], [`futex`]: the property areas as shared files
//!   the guest maps at `/dev/__properties__`, the `property_service`
//!   sockets, and the cross-process futex wake for property readers.
//! - [`supervisor`], [`launch`], [`identity`]: services as host processes
//!   running `linux-run`, with their sockets, environment, credentials and
//!   restart policies.
//! - [`fsops`], [`paths`], [`guest_inode`]: filesystem builtins against
//!   the writable areas of the guest view, and the guest owners of their
//!   files.
//! - [`apex`]: `/apex/apex-info-list.xml` for the pre-flattened APEX tree.
//! - [`executor`], [`boot`]: the `CommandExecutor` and init's main loop;
//!   [`mount`]: the data image, attached while the boot starts.
//! - [`mac`]: the Mac's time zone, language and keyboard layout as
//!   `vendor.aim.mac.*` properties.
//! - [`sku`]: the SKU a bootloader would report, from the Mac's sensors.
//!
//! What the syscall layer must provide for this to work is specified in
//! `docs/guest-init-contract.md`.

pub mod apex;
pub mod boot;
pub mod executor;
pub mod fsops;
pub mod futex;
pub mod guest_inode;
pub mod identity;
pub mod launch;
pub mod mac;
pub mod mount;
pub mod mount_namespace;
pub mod paths;
pub mod props;
pub mod propsvc;
pub mod sku;
pub mod supervisor;
pub mod service_namespace;
pub mod unixsock;

pub use boot::{Boot, BootOptions, BootReport, RunMode};
