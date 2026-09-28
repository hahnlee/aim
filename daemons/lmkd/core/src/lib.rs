//! lmkd without its pressure source: the control protocol ActivityManager
//! speaks (`lmkd.h`), the registered processes and the kill policy.
//!
//! [`policy`] decides whom to kill; [`server`] serves the socket, watches a
//! [`server::Pressure`] source and kills. The guest daemon (`daemons/lmkd`)
//! plugs in the Mac's memory pressure through host-call; tests plug in a
//! simulated one.

pub mod policy;
pub mod proto;
pub mod server;
