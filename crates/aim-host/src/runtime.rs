//! Host-specific composition of the generic runtime owner.
//!
//! Keeping this alias in one module prevents every host subsystem from
//! spelling the four-resource `RuntimeSession` type independently.  More
//! importantly, it gives the next ownership migration one explicit seam:
//! replacing or extending a native owner changes this composition once.

use aim_engine::{EngineSession, GraphicsSession, SurfaceSession};
use aim_runtime::{ProviderBridge, RuntimeSession};

pub(crate) type HostRuntime =
    RuntimeSession<EngineSession, Box<ProviderBridge>, SurfaceSession, GraphicsSession>;
