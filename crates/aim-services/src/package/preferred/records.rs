//! Wire data for NativePreferredRecords.allocate in the original process.
//! The returned identity hash belongs to the actual typed record, not its Binder.
use super::{
    CrossProfileIntentFilter, PersistentPreferredActivity, PreferredActivity, registry::Identity,
};
use aim_binder_host::{
    local::Strong,
    parcel::{Binder, Parcel},
};
use std::sync::Arc;

pub const FORMAT: i32 = 1;
pub const PREFERRED: i32 = 0;
pub const PERSISTENT: i32 = 1;
pub const CROSS_PROFILE: i32 = 2;
pub enum Record<'a> {
    Preferred(&'a PreferredActivity),
    Persistent(&'a PersistentPreferredActivity),
    CrossProfile(&'a CrossProfileIntentFilter),
}
fn component(parcel: &mut Parcel, value: &crate::package::intent::ComponentName) {
    parcel.write_i32(1);
    parcel.write_string16(Some(&value.package));
    parcel.write_string16(Some(&value.class));
}
impl Record<'_> {
    pub fn payload(&self) -> Result<Vec<u8>, String> {
        let mut parcel = Parcel::new();
        parcel.write_i32(FORMAT);
        match self {
            Self::Preferred(record) => {
                parcel.write_i32(PREFERRED);
                parcel.write_i32(1);
                record.filter.write(&mut parcel);
                parcel.write_i32(record.match_);
                match &record.set {
                    None => parcel.write_i32(-1),
                    Some(set) => {
                        parcel.write_i32(
                            i32::try_from(set.len())
                                .map_err(|_| "preferred component set exceeds wire range")?,
                        );
                        for value in set {
                            component(&mut parcel, value);
                        }
                    }
                }
                component(&mut parcel, &record.component);
                parcel.write_bool(record.always);
            }
            Self::Persistent(record) => {
                parcel.write_i32(PERSISTENT);
                parcel.write_i32(1);
                record.filter.write(&mut parcel);
                component(&mut parcel, &record.component);
                parcel.write_bool(record.set_by_dpm);
            }
            Self::CrossProfile(record) => {
                parcel.write_i32(CROSS_PROFILE);
                parcel.write_i32(1);
                record.filter.write(&mut parcel);
                parcel.write_string16(Some(&record.owner_package));
                parcel.write_i32(record.target_user_id);
                parcel.write_i32(record.flags);
                parcel.write_i32(record.access_control);
            }
        }
        if !parcel.objects().is_empty() || !parcel.files().is_empty() {
            return Err("preferred record payload contains capabilities".into());
        }
        Ok(parcel.data().to_vec())
    }
}

/// Concrete capability retained by native records and captured views. A typed
/// producer must construct this after obtaining getIdentityHash from the lease.
pub struct RecordLease {
    strong: Arc<Strong>,
    pub hash: i32,
    pub kind: i32,
}
impl RecordLease {
    pub fn new(strong: Strong, hash: i32, kind: i32) -> Result<Self, String> {
        if !matches!(kind, PREFERRED | PERSISTENT | CROSS_PROFILE) {
            return Err("unknown preferred record lease kind".into());
        }
        Ok(Self {
            strong: Arc::new(strong),
            hash,
            kind,
        })
    }
    pub fn binder(&self) -> Binder {
        self.strong.binder()
    }
    pub fn identity(self: &Arc<Self>) -> Identity {
        Identity {
            hash: self.hash,
            lease: self.clone(),
        }
    }
}
/// Constructor binding contract: allocate each typed record exactly once.
/// Native mutations allocate fresh records; captures retain previous leases.
pub trait Producer: Send + Sync {
    fn allocate_record(&self, record: Record<'_>) -> Result<Arc<RecordLease>, String>;
}

/// System binds its before/after current-bootstrap guard here. Allocation uses
/// the new typed record bridge entry, not allocatePreferredResolverIdentity.
pub struct BridgeProducer {
    bridge: Arc<crate::package::bootstrap::Bridge>,
    current: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
}
impl BridgeProducer {
    pub fn new(
        bridge: Arc<crate::package::bootstrap::Bridge>,
        current: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
    ) -> Self {
        Self { bridge, current }
    }
}
impl Producer for BridgeProducer {
    fn allocate_record(&self, record: Record<'_>) -> Result<Arc<RecordLease>, String> {
        use aim_service_aidl::{
            dev_aim_server_ipackagebootstrapbridge as bootstrap,
            dev_aim_server_ipackageresolveridentity as identity,
        };
        (self.current)()?;
        let kind = match &record {
            Record::Preferred(_) => PREFERRED,
            Record::Persistent(_) => PERSISTENT,
            Record::CrossProfile(_) => CROSS_PROFILE,
        };
        let mut data = Parcel::new();
        bootstrap::AllocatePreferredResolverRecord {
            record: Some(record.payload()?),
        }
        .write(&mut data);
        let reply = self
            .bridge
            .owner
            .transact(bootstrap::ALLOCATE_PREFERRED_RESOLVER_RECORD, &data, false)
            .map_err(|error| format!("preferred record allocation transport {error}"))?;
        let mut reader = reply.reader();
        let binder = bootstrap::read_allocate_preferred_resolver_record_reply(&mut reader)
            .map_err(|error| format!("preferred record reply {error}"))?
            .map_err(|error| format!("preferred record constructor {error:?}"))?
            .ok_or("preferred record lease missing")?;
        if reader.remaining() != 0 {
            return Err("trailing preferred record reply".into());
        }
        let strong = reply
            .retain_remote_binder(binder)
            .map_err(|error| format!("preferred record retention {error}"))?;
        let mut data = Parcel::new();
        identity::GetIdentityHash {}.write(&mut data);
        let reply = strong
            .transact(identity::GET_IDENTITY_HASH, &data, false)
            .map_err(|error| format!("preferred record hash transport {error}"))?;
        let mut reader = reply.reader();
        let hash = identity::read_get_identity_hash_reply(&mut reader)
            .map_err(|error| format!("preferred record hash reply {error}"))?
            .map_err(|error| format!("preferred record hash owner {error:?}"))?;
        if reader.remaining() != 0 {
            return Err("trailing preferred record hash".into());
        }
        (self.current)()?;
        Ok(Arc::new(RecordLease::new(strong, hash, kind)?))
    }
}

impl super::registry::IdentityLease for RecordLease {
    fn typed_record(&self) -> Option<(i32, Binder)> {
        Some((self.kind, self.binder()))
    }
}
impl super::registry::IdentityProvider for BridgeProducer {
    fn allocate(&self, record: Record<'_>) -> Result<Identity, String> {
        self.allocate_record(record).map(|record| record.identity())
    }
}
