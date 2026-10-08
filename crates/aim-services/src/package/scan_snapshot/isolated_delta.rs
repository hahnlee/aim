//! Isolated owner changes affect runtime identity only. They never replace a
//! newer canonical scan with an older query projection during installation.
use super::*;
impl Capture {
    pub(crate) fn with_isolated_owner_delta(
        self: &Arc<Self>,
        isolated: i32,
        owner: Option<i32>,
    ) -> Result<Arc<Self>, String> {
        let old = self
            .state
            .system
            .isolated_owners
            .iter()
            .find(|(uid, _)| *uid == isolated)
            .map(|(_, owner)| *owner);
        if old == owner {
            return Ok(self.clone());
        }
        let mut context = (*self.context).clone();
        let mut state = (*self.state).clone();
        match owner {
            Some(owner) => {
                if let Some((_, old)) = context
                    .system
                    .isolated_owners
                    .iter_mut()
                    .find(|(uid, _)| *uid == isolated)
                {
                    *old = owner;
                } else {
                    context.system.isolated_owners.push((isolated, owner));
                    context.system.isolated_owners.sort_by_key(|(uid, _)| *uid);
                }
            }
            None => context
                .system
                .isolated_owners
                .retain(|(uid, _)| *uid != isolated),
        }
        state.system.isolated_owners = context.system.isolated_owners.clone();
        Ok(Arc::new(Self {
            scan: self.scan.clone(),
            state: Arc::new(state),
            context: Arc::new(context),
            domains: self.domains.clone(),
            frozen: self.frozen.clone(),
            resolver: Default::default(),
        }))
    }
    pub(crate) fn prepare_isolated_owner_delta(
        self: &Arc<Self>,
        isolated: i32,
        owner: Option<i32>,
    ) -> Result<PackageUpdate, String> {
        let projected = self.with_isolated_owner_delta(isolated, owner)?;
        let version = self
            .scan
            .version()
            .checked_add(1)
            .ok_or("isolated owner version exhausted")?;
        let store = super::super::Store::new_replica_at_version(
            self.scan.owner().clone(),
            self.scan.usage().clone(),
            version,
        )
        .map_err(|error| format!("isolated owner replica: {error:?}"))?;
        let mut context = (*projected.context).clone();
        context.scan_version = version;
        let mut state = (*projected.state).clone();
        state.generation = version;
        let capture = Arc::new(Self {
            scan: store.capture(),
            state: Arc::new(state),
            context: Arc::new(context),
            domains: projected.domains.clone(),
            frozen: projected.frozen.clone(),
            resolver: Default::default(),
        });
        Ok(PackageUpdate { store, capture })
    }
}
