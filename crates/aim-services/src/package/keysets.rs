//! KeySetManagerService Binder identities at android-16.0.0_r1.
//! Adapted from AOSP, Copyright The Android Open Source Project, Apache-2.0.
use aim_binder_host::{
    local::{Call, LocalProcess, Reply, Service},
    parcel::{Binder, Parcel, Reader, Result as ParcelResult, UNKNOWN_TRANSACTION},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// One native PMS lifetime, shared by every retained query generation.
pub struct Tokens {
    process: Arc<LocalProcess>,
    handles: Mutex<BTreeMap<i64, Binder>>,
}
impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeySetTokens").finish_non_exhaustive()
    }
}
impl PartialEq for Tokens {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}
impl Tokens {
    pub fn new(process: Arc<LocalProcess>) -> Self {
        Self {
            process,
            handles: Mutex::new(BTreeMap::new()),
        }
    }
    pub fn token(&self, id: i64) -> Binder {
        *self
            .handles
            .lock()
            .unwrap()
            .entry(id)
            .or_insert_with(|| self.process.add_service(Arc::new(Handle)))
    }
    pub fn id(&self, token: Binder) -> Option<i64> {
        self.handles
            .lock()
            .unwrap()
            .iter()
            .find_map(|(id, binder)| (*binder == token).then_some(*id))
    }
}
struct Handle;
impl Service for Handle {
    fn descriptor(&self) -> &str {
        ""
    }
    fn has_descriptor(&self) -> bool {
        false
    }
    fn transact(&self, _: &mut Call<'_>) -> Reply {
        Err(UNKNOWN_TRANSACTION)
    }
}
/// KeySet.writeToParcel writes its Binder capability, never the persisted ID.
#[derive(Clone, Copy, Debug)]
pub struct KeySet {
    pub token: Option<Binder>,
}
impl aim_service_aidl::ReadParcelable for KeySet {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(Self {
            token: r.read_binder()?,
        })
    }
}
impl aim_service_aidl::WriteParcelable for KeySet {
    fn write_to(&self, p: &mut Parcel) {
        p.write_binder(self.token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn descriptorless_keyset_node_is_a_real_binder_capability() {
        let driver = aim_binder_driver::Driver::new();
        let process = LocalProcess::open(
            &driver,
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: 88033,
                euid: 1000,
                security_context: None,
            },
        );
        struct Cleanup(Arc<aim_binder_driver::Driver>, Arc<LocalProcess>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                self.0.release(self.1.proc_handle());
            }
        }
        let _cleanup = Cleanup(driver, process.clone());
        let tokens = Tokens::new(process.clone());
        let token = tokens.token(19);
        let Binder::Local(ptr) = token else {
            panic!("token has no owned Binder node")
        };
        let local = process.local_service(ptr).unwrap();
        let reply = local
            .transact(u32::from_be_bytes(*b"_NTF"), &Parcel::new(), false)
            .unwrap();
        assert_eq!(reply.reader().read_string16().unwrap(), None);
        assert!(
            local
                .transact(u32::from_be_bytes(*b"_PNG"), &Parcel::new(), false)
                .is_ok()
        );
        assert_eq!(
            local.transact(1, &Parcel::new(), false).err(),
            Some(UNKNOWN_TRANSACTION)
        );
        assert_eq!(tokens.id(token), Some(19));
        assert_eq!(tokens.token(19), token);
        assert_ne!(Tokens::new(process).token(19), token);
    }
}
