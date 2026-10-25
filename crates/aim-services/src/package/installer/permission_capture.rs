//! Full native computer transport for an original permission-owner transaction.
use crate::package::scan_snapshot::{
    Snapshot,
    endpoint::Endpoint,
    query_state::{Capture, Context},
};
use aim_binder_host::{local::LocalProcess, parcel::Binder};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock, Weak},
};

/// Root supplies the actual external owners for precisely this candidate.
pub type ContextSource = Arc<dyn Fn(&Arc<Snapshot>) -> Result<Context, String> + Send + Sync>;

// Exact local Binder capabilities, never a package-name or version lookup.
fn scopes() -> &'static Mutex<BTreeMap<(usize, u64), Weak<Endpoint>>> {
    static SCOPES: OnceLock<Mutex<BTreeMap<(usize, u64), Weak<Endpoint>>>> = OnceLock::new();
    SCOPES.get_or_init(Default::default)
}
pub(crate) fn bind(process: &Arc<LocalProcess>, endpoint: &Arc<Endpoint>) -> Binder {
    let binder = process.add_service(endpoint.clone());
    let Binder::Local(ptr) = binder else {
        unreachable!()
    };
    let mut scopes = scopes().lock().unwrap();
    scopes.retain(|_, scope| scope.strong_count() != 0);
    scopes.insert(
        (Arc::as_ptr(process) as usize, ptr),
        Arc::downgrade(endpoint),
    );
    binder
}
pub(crate) fn local_scope(
    process: &Arc<LocalProcess>,
    binder: Binder,
) -> Result<Arc<Endpoint>, String> {
    let Binder::Local(ptr) = binder else {
        return Err("permission scope is not local".into());
    };
    scopes()
        .lock()
        .unwrap()
        .get(&(Arc::as_ptr(process) as usize, ptr))
        .and_then(Weak::upgrade)
        .ok_or_else(|| "permission scope is retired".into())
}

pub fn endpoint(
    snapshot: Arc<Snapshot>,
    context: Context,
    process: &Arc<LocalProcess>,
) -> Result<Arc<Endpoint>, String> {
    let capture = Capture::new(snapshot.clone(), context)?;
    Ok(Arc::new(Endpoint::with_computer(
        snapshot, capture, process,
    )))
}
