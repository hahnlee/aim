//! The `settings` provider for the app host: `IContentProvider.call` of
//! `SettingsProvider` at `android-16.0.0_r1` for what apps read
//! (`GET_system`, `GET_secure`, `GET_global`, `GET_config`, `LIST_config`),
//! from the settings the app host loaded (`super::settings`), and the
//! `ContentProviderHolder` ActivityManager hands out for it.
//!
//! Replies carry no generation tracking (`_track_generation`): clients
//! then do not cache values and ask again (#1430).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Exception, Parcel, Reader};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_content_icontentprovider as cp;

use super::settings::Table;
use super::{AppHost, reply};
use crate::bundle::{self, Value};
use crate::package::info::ProviderInfo;

/// `Settings.NameValueTable.VALUE`, `Settings.CALL_METHOD_PREFIX_KEY`.
const VALUE: &str = "value";
const PREFIX: &str = "_prefix";

pub struct SettingsProvider(pub Arc<AppHost>);

impl Service for SettingsProvider {
    fn descriptor(&self) -> &str {
        cp::DESCRIPTOR
    }

    fn accepts_fds(&self) -> bool {
        // A call's extras may carry descriptors (none are read).
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != cp::CALL_TRANSACTION {
            return reply(Err(self.0.unimplemented("settings provider", call.code)));
        }
        let r = &mut call.data;
        r.enforce_interface(cp::DESCRIPTOR).map_err(|_| BAD_VALUE)?;
        skip_structured(r)?; // the caller's `AttributionSource`
        let _authority = r.read_string16().map_err(|_| BAD_VALUE)?;
        let method = r.read_string16().map_err(|_| BAD_VALUE)?.ok_or(BAD_VALUE)?;
        let arg = r.read_string16().map_err(|_| BAD_VALUE)?;
        let extras = bundle::read(r).map_err(|_| BAD_VALUE)?.unwrap_or_default();
        reply(self.call(&method, arg.as_deref(), &extras).map(|result| {
            let mut p = Parcel::new();
            p.write_no_exception();
            bundle::write(
                &mut p,
                &result
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.clone()))
                    .collect::<Vec<_>>(),
            );
            p
        }))
    }
}

impl SettingsProvider {
    /// `SettingsProvider.call`.
    fn call(
        &self,
        method: &str,
        arg: Option<&str>,
        extras: &std::collections::HashMap<String, Value>,
    ) -> Result<Vec<(String, Value)>, Exception> {
        let inputs = self.0.inputs()?;
        let settings = &inputs.settings;
        let get = |table| {
            vec![(
                VALUE.to_string(),
                Value::String(
                    arg.and_then(|name| settings.get(table, name))
                        .map(str::to_string),
                ),
            )]
        };
        Ok(match method {
            "GET_system" => get(Table::System),
            "GET_secure" => get(Table::Secure),
            "GET_global" => get(Table::Global),
            "GET_config" => get(Table::Config),
            "LIST_config" => {
                let prefix = match extras.get(PREFIX) {
                    Some(Value::String(prefix)) => prefix.clone().unwrap_or_default(),
                    _ => String::new(),
                };
                vec![(
                    VALUE.to_string(),
                    Value::StringMap(settings.list(Table::Config, &prefix)),
                )]
            }
            other => return Err(self.0.unimplemented_named("settings provider", other)),
        })
    }
}

/// A structured parcelable (its size, then its fields) left unread.
fn skip_structured(r: &mut Reader<'_>) -> Result<(), i32> {
    let start = r.position();
    let size = usize::try_from(r.read_i32().map_err(|_| BAD_VALUE)?).map_err(|_| BAD_VALUE)?;
    if size < 4 || size - 4 > r.remaining() {
        return Err(BAD_VALUE);
    }
    r.set_position(start + size);
    Ok(())
}

/// `ContentProviderHolder` of a system provider: no release needed.
pub struct ContentProviderHolder {
    pub info: ProviderInfo,
    pub provider: Binder,
    pub connection: Binder,
}

impl WriteParcelable for ContentProviderHolder {
    fn write_to(&self, p: &mut Parcel) {
        self.info.write_to(p);
        p.write_binder(Some(self.provider));
        p.write_binder(Some(self.connection));
        p.write_i32(1); // no release needed
        p.write_i32(0); // not local
    }
}
