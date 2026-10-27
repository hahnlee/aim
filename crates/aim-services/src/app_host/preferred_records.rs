//! The preferred resolver records system_server's `NativePreferredRecords`
//! allocates for the native package boot (`android-16.0.0_r1`): a typed
//! record read from its original Parcel, as the original constructs it,
//! leased as an `IPackageResolverIdentity` whose hash is the record's
//! identity, to the system uid only.

use std::sync::atomic::{AtomicI32, Ordering};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_BAD_PARCELABLE, EX_NULL_POINTER, Exception, Parcel, Reader, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::dev_aim_server_ipackageresolveridentity as identity;

use super::reply;
use crate::package::intent_filter::{IntentFilter, Plain};
use crate::package::preferred::records::{CROSS_PROFILE, FORMAT, PERSISTENT, PREFERRED};

const SYSTEM_UID: u32 = 1000;

/// `System.identityHashCode`: a value of each object's own, for as long
/// as it lives.
fn identity_hash() -> i32 {
    static NEXT: AtomicI32 = AtomicI32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A record's lease (`Lease`), or the bridge's plain identity
/// (`allocatePreferredResolverIdentity`).
pub struct Lease {
    hash: i32,
    message: &'static str,
}

impl Lease {
    /// `allocatePreferredResolverIdentity`'s identity.
    pub fn identity() -> Self {
        Lease {
            hash: identity_hash(),
            message: "the package bridge serves the system uid only",
        }
    }
}

impl Service for Lease {
    fn descriptor(&self) -> &str {
        identity::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            identity::GET_IDENTITY_HASH => {
                identity::GetIdentityHash::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(if call.sender_euid == SYSTEM_UID {
                    let mut p = Parcel::new();
                    identity::write_get_identity_hash_reply(&mut p, self.hash);
                    Ok(p)
                } else {
                    Err(Exception::security(self.message))
                })
            }
            _ => Err(UNKNOWN_TRANSACTION),
        }
    }
}

fn npe(message: &str) -> Exception {
    Exception::new(EX_NULL_POINTER, message)
}

/// `readTypedObject(ComponentName.CREATOR)`, which `ComponentName(Parcel)`
/// refuses without its names.
fn component(r: &mut Reader<'_>) -> Result<bool, Exception> {
    let bad = |_| Exception::new(EX_BAD_PARCELABLE, "preferred record component");
    if r.read_i32().map_err(bad)? == 0 {
        return Ok(false);
    }
    if r.read_string16().map_err(bad)?.is_none() {
        return Err(npe("package name is null"));
    }
    if r.read_string16().map_err(bad)?.is_none() {
        return Err(npe("class name is null"));
    }
    Ok(true)
}

/// `NativePreferredRecords.allocate`: the record's Parcel read whole, then
/// leased.
pub fn allocate(bytes: Option<Vec<u8>>) -> Result<Lease, Exception> {
    let bytes = bytes.ok_or_else(|| npe("preferred record payload"))?;
    let mut r = Reader::new(&bytes, &[]);
    let bad = |_| Exception::new(EX_BAD_PARCELABLE, "preferred record payload");
    if r.read_i32().map_err(bad)? != FORMAT {
        return Err(Exception::illegal_argument("preferred record version"));
    }
    let kind = r.read_i32().map_err(bad)?;
    if r.read_i32().map_err(bad)? == 0 {
        return Err(npe("preferred record filter"));
    }
    IntentFilter::read(&mut r, &mut Plain).map_err(bad)?;
    match kind {
        PREFERRED => {
            r.read_i32().map_err(bad)?;
            let set = r.read_i32().map_err(bad)?;
            for _ in 0..set.max(0) {
                component(&mut r)?;
            }
            // `PreferredComponent` flattens its activity's name.
            if !component(&mut r)? {
                return Err(npe("preferred record activity"));
            }
            r.read_i32().map_err(bad)?;
        }
        PERSISTENT => {
            component(&mut r)?;
            r.read_i32().map_err(bad)?;
        }
        CROSS_PROFILE => {
            r.read_string16().map_err(bad)?;
            for _ in 0..3 {
                r.read_i32().map_err(bad)?;
            }
        }
        _ => {
            return Err(Exception::illegal_argument(format!(
                "preferred record kind {kind}"
            )));
        }
    }
    // `enforceNoDataAvail`.
    if r.remaining() != 0 {
        return Err(Exception::new(
            EX_BAD_PARCELABLE,
            format!(
                "Parcel data not fully consumed, unread size: {}",
                r.remaining()
            ),
        ));
    }
    Ok(Lease {
        hash: identity_hash(),
        message: "native preferred record identities require system UID",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::intent::ComponentName;
    use crate::package::preferred::records::Record;
    use crate::package::preferred::{PersistentPreferredActivity, PreferredActivity};
    use aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT;

    fn name(class: &str) -> ComponentName {
        ComponentName {
            package: "p".into(),
            class: class.into(),
        }
    }

    /// The records the native registry writes are read whole, each with an
    /// identity of its own; a malformed one is refused as the original
    /// refuses it.
    #[test]
    fn native_records_are_leased() {
        let preferred = PreferredActivity {
            filter: IntentFilter::default(),
            component: name("A"),
            match_: 0x0010_0000,
            always: true,
            set: Some(vec![name("A"), name("B")]),
        };
        let persistent = PersistentPreferredActivity {
            filter: IntentFilter::default(),
            component: name("A"),
            set_by_dpm: false,
        };
        let a = allocate(Some(Record::Preferred(&preferred).payload().unwrap())).unwrap();
        let b = allocate(Some(Record::Persistent(&persistent).payload().unwrap())).unwrap();
        assert_ne!(a.hash, b.hash);
        let mut extra = Record::Persistent(&persistent).payload().unwrap();
        extra.extend_from_slice(&[0; 4]);
        assert_eq!(allocate(Some(extra)).err().unwrap().code, EX_BAD_PARCELABLE);
        let mut version = Record::Persistent(&persistent).payload().unwrap();
        version[0] = 2;
        assert_eq!(
            allocate(Some(version)).err().unwrap().code,
            EX_ILLEGAL_ARGUMENT
        );
        assert_eq!(allocate(None).err().unwrap().code, EX_NULL_POINTER);
    }
}
