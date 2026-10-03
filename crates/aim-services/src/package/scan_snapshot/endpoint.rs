//! Private Binder lease on one captured scan owner, for facade code transport.
use super::Snapshot;
use crate::package::pkg::FacadeEntry;
use aim_binder_host::{
    local::{Call, Reply, Service},
    parcel::{EX_ILLEGAL_STATE, Exception, Parcel, UNKNOWN_TRANSACTION},
};
use aim_service_aidl::{
    WriteParcelable, dev_aim_server_ipackagescansnapshot as api, write_byte_array, write_long_array,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

pub struct PackageCode {
    version: u64,
    name: String,
    entry: FacadeEntry,
}

impl PackageCode {
    pub fn captured(
        snapshot: &Snapshot,
        name: &str,
        disabled: bool,
    ) -> Result<Option<Self>, String> {
        let owner = snapshot.owner();
        let loaded = if disabled {
            owner.disabled_loaded_packages()
        } else {
            owner.loaded_packages()
        };
        let Some(code) = loaded.get(name) else {
            return Ok(None);
        };
        Ok(Some(Self {
            version: snapshot.version(),
            name: name.into(),
            entry: code.facade_entry()?,
        }))
    }
}

impl WriteParcelable for PackageCode {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i64(self.version as i64);
        p.write_string16(Some(&self.name));
        write_byte_array(p, Some(&self.entry.cache.bytes));
        match &self.entry.past_signing_certificates {
            None => p.write_i32(-1),
            Some(past) => {
                p.write_i32(past.len() as i32);
                for (certificate, flags) in past {
                    write_byte_array(p, Some(certificate));
                    p.write_i32(*flags);
                }
            }
        }
    }
}

pub struct PackageUsage {
    version: u64,
    name: String,
    historical: bool,
    times: [i64; super::super::owner::usage::REASONS],
}

impl PackageUsage {
    pub fn captured(snapshot: &Snapshot, name: &str) -> Option<Self> {
        Some(Self {
            version: snapshot.version(),
            name: name.into(),
            historical: snapshot.usage().historical_available(),
            times: *snapshot.usage().times(name)?,
        })
    }
}

impl WriteParcelable for PackageUsage {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i64(self.version as i64);
        p.write_string16(Some(&self.name));
        p.write_bool(self.historical);
        write_long_array(p, Some(&self.times));
    }
}

pub const MAX_CHUNK: usize = 64 * 1024;

struct Lease {
    snapshot: Option<Arc<Snapshot>>,
    code: BTreeMap<(String, bool), Arc<Vec<u8>>>,
}

impl Lease {
    fn code(
        &mut self,
        snapshot: &Snapshot,
        name: Option<&str>,
        disabled: bool,
    ) -> Result<Option<Arc<Vec<u8>>>, Exception> {
        let name = name.ok_or_else(|| Exception::illegal_argument("package name is null"))?;
        let key = (name.to_owned(), disabled);
        if let Some(code) = self.code.get(&key) {
            return Ok(Some(code.clone()));
        }
        let code = PackageCode::captured(snapshot, name, disabled)
            .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error))?;
        let Some(code) = code else {
            return Ok(None);
        };
        let mut parcel = Parcel::new();
        code.write_to(&mut parcel);
        let bytes = Arc::new(parcel.data().to_vec());
        if bytes.len() > i32::MAX as usize {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "package code exceeds transport size",
            ));
        }
        self.code.insert(key, bytes.clone());
        Ok(Some(bytes))
    }
}

pub struct Endpoint {
    lease: Mutex<Lease>,
}

impl Endpoint {
    pub fn new(snapshot: Arc<Snapshot>) -> Self {
        Self {
            lease: Mutex::new(Lease {
                snapshot: Some(snapshot),
                code: BTreeMap::new(),
            }),
        }
    }
}

impl Service for Endpoint {
    fn descriptor(&self) -> &str {
        api::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if !api::METHODS.iter().any(|(code, _)| *code == call.code) {
            return Err(UNKNOWN_TRANSACTION);
        }
        let mut reply = Parcel::new();
        if call.sender_euid != crate::SYSTEM_UID {
            reply.write_exception(&Exception::security(
                "package scan snapshot serves system uid only",
            ));
            return Ok(reply);
        }
        // Hold this reference through the response, even if close races the call.
        let start = call.data.position();
        call.data.enforce_interface(api::DESCRIPTOR)?;
        call.data.set_position(start);
        let mut lease = self.lease.lock().unwrap();
        if call.code == api::CLOSE {
            api::Close::read(&mut call.data)?;
            if call.data.remaining() != 0 {
                return Err(aim_binder_host::parcel::BAD_VALUE);
            }
            lease.snapshot.take();
            lease.code.clear();
            reply.write_no_exception();
            return Ok(reply);
        }
        let Some(snapshot) = lease.snapshot.clone() else {
            reply.write_exception(&Exception::new(
                EX_ILLEGAL_STATE,
                "package scan snapshot is closed",
            ));
            return Ok(reply);
        };
        match call.code {
            api::GET_VERSION => {
                api::GetVersion::read(&mut call.data)?;
                if call.data.remaining() != 0 {
                    return Err(aim_binder_host::parcel::BAD_VALUE);
                }
                api::write_get_version_reply(&mut reply, snapshot.version() as i64);
            }
            api::GET_PACKAGE_NAMES => {
                let disabled = api::GetPackageNames::read(&mut call.data)?.disabled;
                if call.data.remaining() != 0 {
                    return Err(aim_binder_host::parcel::BAD_VALUE);
                }
                let settings = if disabled {
                    &snapshot.owner().settings.disabled_system_packages
                } else {
                    &snapshot.owner().settings.packages
                };
                let names = Some(settings.iter().map(|p| Some(p.name.clone())).collect());
                api::write_get_package_names_reply(&mut reply, &names);
            }
            api::GET_USAGE => {
                let args = api::GetUsage::read(&mut call.data)?;
                if call.data.remaining() != 0 {
                    return Err(aim_binder_host::parcel::BAD_VALUE);
                }
                match args.package_name.as_deref() {
                    None => {
                        reply.write_exception(&Exception::illegal_argument("package name is null"))
                    }
                    Some(name) => {
                        let bytes = PackageUsage::captured(&snapshot, name).map(|usage| {
                            let mut parcel = Parcel::new();
                            usage.write_to(&mut parcel);
                            parcel.data().to_vec()
                        });
                        api::write_get_usage_reply(&mut reply, &bytes);
                    }
                }
            }
            api::GET_CODE_LENGTH => {
                let args = api::GetCodeLength::read(&mut call.data)?;
                if call.data.remaining() != 0 {
                    return Err(aim_binder_host::parcel::BAD_VALUE);
                }
                match lease.code(&snapshot, args.package_name.as_deref(), args.disabled) {
                    Ok(code) => api::write_get_code_length_reply(
                        &mut reply,
                        code.map_or(-1, |code| code.len() as i32),
                    ),
                    Err(error) => reply.write_exception(&error),
                }
            }
            api::GET_CODE_CHUNK => {
                let args = api::GetCodeChunk::read(&mut call.data)?;
                if call.data.remaining() != 0 {
                    return Err(aim_binder_host::parcel::BAD_VALUE);
                }
                if args.offset < 0 || args.length <= 0 || args.length as usize > MAX_CHUNK {
                    reply.write_exception(&Exception::illegal_argument(
                        "invalid package code chunk range",
                    ));
                } else {
                    match lease.code(&snapshot, args.package_name.as_deref(), args.disabled) {
                        Ok(None) => api::write_get_code_chunk_reply(&mut reply, &None),
                        Ok(Some(code)) if args.offset as usize > code.len() => reply
                            .write_exception(&Exception::illegal_argument(
                                "package code offset exceeds length",
                            )),
                        Ok(Some(code)) => {
                            let start = args.offset as usize;
                            let end = code.len().min(start + args.length as usize);
                            api::write_get_code_chunk_reply(
                                &mut reply,
                                &Some(code[start..end].to_vec()),
                            );
                        }
                        Err(error) => reply.write_exception(&error),
                    }
                }
            }
            _ => unreachable!(),
        }
        Ok(reply)
    }
}
