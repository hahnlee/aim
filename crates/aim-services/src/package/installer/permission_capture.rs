//! Full native computer transport for an original permission-owner transaction.
use crate::package::scan_snapshot::{Snapshot, endpoint::Endpoint, query_state::{Capture, Context}};
use aim_binder_host::local::LocalProcess;
use std::sync::Arc;

/// Root supplies the actual external owners for precisely this candidate,
/// including users, domain state and live permission inputs. Capture validates
/// every package's identity before Java can enter its transaction scope.
pub type ContextSource = Arc<dyn Fn(&Arc<Snapshot>) -> Result<Context, String> + Send + Sync>;

pub fn endpoint(snapshot: Arc<Snapshot>, context: Context,
    process: &Arc<LocalProcess>) -> Result<Arc<Endpoint>, String> {
    let capture = Capture::new(snapshot.clone(), context)?;
    Ok(Arc::new(Endpoint::with_computer(snapshot, capture, process)))
}
