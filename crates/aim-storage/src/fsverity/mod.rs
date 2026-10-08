//! Linux fs-verity data proof and durable inode metadata (#226).
//! ENABLE is not exposed here: callers must retain filesystem admission leases.
mod tree;
mod store;
pub use tree::{BuildOptions, Descriptor, build};
pub use store::{Admission,EnableGuard,Metadata,Prepared,Store};
pub use crate::inode_lease::Identity;
use std::io;

#[derive(Debug)]
pub enum Error { Linux(i32), Io(io::Error) }
impl From<io::Error> for Error { fn from(error:io::Error)->Self{Self::Io(error)} }
pub type Result<T> = std::result::Result<T,Error>;
pub const EIO:i32=5;
pub const EINVAL:i32=22;
pub const ENODATA:i32=61;
pub const EOVERFLOW:i32=75;
pub const EMSGSIZE:i32=90;
