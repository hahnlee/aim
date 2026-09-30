//! The settings provider, read and written as system_server's own code
//! does (`Settings.Secure`, `DeviceConfig`): `IContentProvider.call` on
//! the provider `IActivityManager.getContentProviderExternal` hands out.
//!
//! Values are kept the way the framework's settings clients keep them
//! (`Settings.NameValueCache`): each read asks the provider to track its
//! generation, and the provider answers with a shared array of generation
//! numbers (`MemoryIntArray`, ashmem) and the value's index in it, which it
//! bumps on every change. A kept value is used while its generation is
//! the one it came with, so a decision reads memory, not the provider,
//! and never uses a value older than the last change. A `DeviceConfig`
//! namespace is tracked as a whole (`LIST_config` of its prefix), as the
//! provider bumps it for any of its properties. Everything kept goes
//! with the provider when system_server dies.

use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, LocalProcess, Received, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Exception, Parcel};
use aim_service_aidl::android_content_icontentprovider as provider;

use crate::bundle::{self, Value};
use crate::system::System;

type Result<T> = std::result::Result<T, Exception>;

/// `Settings.AUTHORITY`.
const AUTHORITY: &str = "settings";
/// `Settings.CALL_METHOD_*` and their keys.
const GET_CONFIG: &str = "GET_config";
const LIST_CONFIG: &str = "LIST_config";
const GET_SECURE: &str = "GET_secure";
const GET_GLOBAL: &str = "GET_global";
const PUT_SECURE: &str = "PUT_secure";
const USER_KEY: &str = "_user";
const PREFIX_KEY: &str = "_prefix";
const TRACK_GENERATION_KEY: &str = "_track_generation";
const GENERATION_INDEX_KEY: &str = "_generation_index";
const GENERATION_KEY: &str = "_generation";
/// `Settings.NameValueTable.VALUE`.
const VALUE: &str = "value";
/// `Process.INVALID_PID`: the caller's pid, which the provider takes from
/// the driver.
const INVALID_PID: i32 = -1;
const SYSTEM_UID: i32 = 1000;
/// The package system_server's context calls as.
const SYSTEM_PACKAGE: &str = "android";
/// `UserHandle.USER_SYSTEM`: `DeviceConfig` is not per user.
const USER_SYSTEM: i32 = 0;

fn failed(what: &str, status: i32) -> Exception {
    Exception::new(
        aim_binder_host::parcel::EX_ILLEGAL_STATE,
        format!("settings {what}: binder status {status}"),
    )
}

/// A provider's `MemoryIntArray`, mapped read-only.
struct Generations {
    base: *const AtomicI32,
    len: usize,
}

// SAFETY: a read-only shared mapping, read with atomic loads.
unsafe impl Send for Generations {}
unsafe impl Sync for Generations {}

impl Generations {
    fn map(file: &aim_binder_driver::File) -> Option<Generations> {
        let fd = aim_binder_host::server::file_fd(file)?;
        // SAFETY: fstat and a read-only shared mapping of a file we hold
        // open; the mapping outlives the fd.
        unsafe {
            let mut st: libc::stat = std::mem::zeroed();
            if libc::fstat(fd.as_raw_fd(), &mut st) != 0 || st.st_size < 4 {
                return None;
            }
            let bytes = st.st_size as usize;
            let p = libc::mmap(
                std::ptr::null_mut(),
                bytes,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            );
            (p != libc::MAP_FAILED).then(|| Generations {
                base: p.cast(),
                len: bytes / 4,
            })
        }
    }

    /// `MemoryIntArray.get(index)`.
    fn get(&self, index: usize) -> Option<i32> {
        // SAFETY: inside the mapping.
        (index < self.len).then(|| unsafe { (*self.base.add(index)).load(Ordering::Acquire) })
    }
}

impl Drop for Generations {
    fn drop(&mut self) {
        // SAFETY: the mapping made in `map`.
        unsafe { libc::munmap(self.base as *mut _, self.len * 4) };
    }
}

/// Where a value's generation is, and the one it was read at.
#[derive(Clone)]
struct Generation {
    array: Arc<Generations>,
    index: usize,
    value: i32,
}

impl Generation {
    fn current(&self) -> bool {
        self.array.get(self.index) == Some(self.value)
    }
}

/// A settings table's key: the call that reads it, the user and the name
/// (the namespace, for `DeviceConfig`).
type Key = (&'static str, i32, String);

#[derive(Default)]
struct Kept {
    /// The provider, and the cookie of its death notification.
    provider: Option<(Arc<Strong>, u64)>,
    /// `Settings.Secure` values with their generations.
    values: HashMap<Key, (Option<String>, Generation)>,
    /// `DeviceConfig` namespaces: their generation and the values read
    /// at it.
    namespaces: HashMap<String, (Generation, HashMap<String, Option<String>>)>,
}

pub struct Settings {
    process: Arc<LocalProcess>,
    system: Arc<System>,
    /// The token the provider reference is held for.
    token: Binder,
    kept: Mutex<Kept>,
    this: Weak<Settings>,
}

/// A node that stands for this process's reference to the provider.
struct Token;

impl Service for Token {
    fn descriptor(&self) -> &str {
        ""
    }

    fn transact(&self, _: &mut Call<'_>) -> Reply {
        Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION)
    }
}

impl Settings {
    pub fn new(process: Arc<LocalProcess>, system: Arc<System>) -> Arc<Self> {
        let token = process.add_service(Arc::new(Token));
        Arc::new_cyclic(|this| Settings {
            process,
            system,
            token,
            kept: Mutex::new(Kept::default()),
            this: this.clone(),
        })
    }

    /// The settings provider, acquired once and dropped with everything
    /// kept when it dies.
    fn provider(&self) -> Result<Arc<Strong>> {
        if let Some((p, _)) = &self.kept.lock().unwrap().provider {
            return Ok(p.clone());
        }
        let strong = Arc::new(
            self.system
                .content_provider_external(AUTHORITY, USER_SYSTEM, self.token, "native services")?
                .ok_or_else(|| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "no settings provider",
                    )
                })?,
        );
        let this = self.this.clone();
        let death = self.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(settings) = this.upgrade() {
                    *settings.kept.lock().unwrap() = Kept::default();
                }
            }),
        );
        let mut kept = self.kept.lock().unwrap();
        if let Some((p, _)) = &kept.provider {
            self.process.clear_death(&strong, death);
            return Ok(p.clone());
        }
        kept.provider = Some((strong.clone(), death));
        Ok(strong)
    }

    /// `IContentProvider.call(attributionSource, "settings", method, arg,
    /// extras)`: the reply's bundle, with the file of its generation
    /// array when it has one.
    fn call(
        &self,
        method: &str,
        arg: Option<&str>,
        extras: &[(&str, Value)],
    ) -> Result<(HashMap<String, Value>, Option<Generation>)> {
        let provider = self.provider()?;
        let mut data = Parcel::new();
        data.write_interface_token(provider::DESCRIPTOR);
        write_attribution_source(&mut data);
        data.write_string16(Some(AUTHORITY));
        data.write_string16(Some(method));
        data.write_string16(arg);
        bundle::write(&mut data, extras);
        let reply = provider
            .transact(provider::CALL_TRANSACTION, &data, false)
            .map_err(|s| failed(method, s))?;
        let mut r = reply.reader();
        r.read_exception().map_err(|s| failed(method, s))??;
        let entries = bundle::read(&mut r)
            .map_err(|s| failed(method, s))?
            .unwrap_or_default();
        let generation = self.generation(&entries, &reply);
        Ok((entries, generation))
    }

    /// The generation data of a reply (`GenerationRegistry.addGenerationData`),
    /// if the provider tracks the value.
    fn generation(&self, entries: &HashMap<String, Value>, reply: &Received) -> Option<Generation> {
        let (Some(Value::Lazy { start, .. }), Some(Value::Int(index)), Some(Value::Int(value))) = (
            entries.get(TRACK_GENERATION_KEY),
            entries.get(GENERATION_INDEX_KEY),
            entries.get(GENERATION_KEY),
        ) else {
            return None;
        };
        // writeParcelable(MemoryIntArray), whose writeToParcel is
        // writeParcelable(ParcelFileDescriptor): 0 (no comm fd), the fd.
        let fd = (|| {
            let mut r = reply.reader();
            r.set_position(*start);
            for class in [
                "android.util.MemoryIntArray",
                "android.os.ParcelFileDescriptor",
            ] {
                if r.read_string16()?.as_deref() != Some(class) {
                    return Err(BAD_VALUE);
                }
            }
            r.read_i32()?;
            r.read_fd()
        })()
        .ok()?;
        let array = Generations::map(&self.process.file(fd)?)?;
        Some(Generation {
            array: Arc::new(array),
            index: usize::try_from(*index).ok()?,
            value: *value,
        })
    }

    /// `Settings.Secure.getStringForUser(resolver, name, user)`.
    pub fn secure(&self, name: &str, user_id: i32) -> Result<Option<String>> {
        self.tracked(GET_SECURE, name, user_id)
    }

    /// `Settings.Global.getString(resolver, name)`.
    pub fn global(&self, name: &str) -> Result<Option<String>> {
        self.tracked(GET_GLOBAL, name, USER_SYSTEM)
    }

    /// A `GET_*` value, kept while its generation is unchanged.
    fn tracked(&self, method: &'static str, name: &str, user_id: i32) -> Result<Option<String>> {
        let key = (method, user_id, name.to_string());
        if let Some((value, generation)) = self.kept.lock().unwrap().values.get(&key)
            && generation.current()
        {
            return Ok(value.clone());
        }
        let (entries, generation) = self.call(
            method,
            Some(name),
            &[
                (USER_KEY, Value::Int(user_id)),
                (TRACK_GENERATION_KEY, Value::Null),
            ],
        )?;
        let value = string(&entries);
        if let Some(generation) = generation {
            self.kept
                .lock()
                .unwrap()
                .values
                .insert(key, (value.clone(), generation));
        }
        Ok(value)
    }

    /// `Settings.Secure.putStringForUser(resolver, name, value, user)`.
    pub fn put_secure(&self, name: &str, value: &str, user_id: i32) -> Result<()> {
        self.call(
            PUT_SECURE,
            Some(name),
            &[
                (VALUE, Value::String(Some(value.into()))),
                (USER_KEY, Value::Int(user_id)),
            ],
        )
        .map(drop)
    }

    /// `DeviceConfig.getProperty(namespace, name)`.
    pub fn config(&self, namespace: &str, name: &str) -> Result<Option<String>> {
        let fresh = {
            let kept = self.kept.lock().unwrap();
            match kept.namespaces.get(namespace) {
                Some((generation, values)) if generation.current() => {
                    if let Some(value) = values.get(name) {
                        return Ok(value.clone());
                    }
                    true
                }
                _ => false,
            }
        };
        if !fresh {
            // The namespace's generation first: a value read after it is
            // at least as new.
            let (_, generation) = self.call(
                LIST_CONFIG,
                None,
                &[
                    (PREFIX_KEY, Value::String(Some(format!("{namespace}/")))),
                    (TRACK_GENERATION_KEY, Value::Null),
                ],
            )?;
            let mut kept = self.kept.lock().unwrap();
            match generation {
                Some(generation) => {
                    kept.namespaces
                        .insert(namespace.to_string(), (generation, HashMap::new()));
                }
                None => {
                    kept.namespaces.remove(namespace);
                }
            }
        }
        let (entries, _) = self.call(
            GET_CONFIG,
            Some(&format!("{namespace}/{name}")),
            &[(USER_KEY, Value::Int(USER_SYSTEM))],
        )?;
        let value = string(&entries);
        if let Some((_, values)) = self.kept.lock().unwrap().namespaces.get_mut(namespace) {
            values.insert(name.to_string(), value.clone());
        }
        Ok(value)
    }

    /// `DeviceConfig.getBoolean`.
    pub fn config_bool(&self, namespace: &str, name: &str, default: bool) -> Result<bool> {
        // Boolean.parseBoolean.
        Ok(self
            .config(namespace, name)?
            .map_or(default, |v| v.eq_ignore_ascii_case("true")))
    }

    /// `DeviceConfig.getLong`: the default for a value that is not a
    /// number.
    pub fn config_long(&self, namespace: &str, name: &str, default: i64) -> Result<i64> {
        Ok(self
            .config(namespace, name)?
            .and_then(|v| v.parse().ok())
            .unwrap_or(default))
    }
}

/// A `GET_*` reply's value.
fn string(entries: &HashMap<String, Value>) -> Option<String> {
    match entries.get(VALUE) {
        Some(Value::String(s)) => s.clone(),
        _ => None,
    }
}

/// `AttributionSource.writeToParcel` of system_server's own source: its
/// `AttributionSourceState` (a stable parcelable: its size, then pid, uid,
/// device id, package, attribution tag, token, renounced permissions and
/// the next sources).
fn write_attribution_source(p: &mut Parcel) {
    let start = p.position();
    p.write_i32(0);
    p.write_i32(INVALID_PID);
    p.write_i32(SYSTEM_UID);
    p.write_i32(0); // Context.DEVICE_ID_DEFAULT
    p.write_string16(Some(SYSTEM_PACKAGE));
    p.write_string16(None);
    p.write_binder(None);
    p.write_i32(-1); // no renounced permissions
    p.write_i32(0); // no next source
    let size = (p.position() - start) as i32;
    p.set_i32_at(start, size);
}
