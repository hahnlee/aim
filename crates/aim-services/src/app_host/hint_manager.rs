//! `performance_hint`: HintManagerService for the app host
//! (`android-16.0.0_r1`), the client data every process's
//! `APerformanceHintManager` registers for (`registerClient`,
//! `getClientData`): the Power HAL's version, preferred rate and support
//! info, as the service reads them at its start. Hint sessions, their
//! threads and channels, headroom and a Power HAL before version 6 are not
//! implemented (#1689).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, OnceLock};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_os_ihintmanager as hm;

use super::{AppHost, reply};

/// The Power HAL (`IPower/default`), its descriptor and the calls the
/// service makes.
const POWER: &str = "android.hardware.power.IPower/default";
const POWER_DESCRIPTOR: &str = "android.hardware.power.IPower";
const GET_HINT_SESSION_PREFERRED_RATE: u32 = 6;
const GET_SUPPORT_INFO: u32 = 10;
/// A stable interface's `getInterfaceVersion`
/// (`IBinder.FIRST_CALL_TRANSACTION + 16777214`).
const GET_INTERFACE_VERSION: u32 = 1 + 16_777_214;
/// `MAX_GRAPHICS_PIPELINE_THREADS_COUNT`,
/// `DEFAULT_MAX_CPU_HEADROOM_THREADS_COUNT`.
const MAX_GRAPHICS_PIPELINE_THREADS: i32 = 5;
const MAX_CPU_HEADROOM_THREADS: i32 = 5;

/// `IHintManager.HintManagerClientData`, with the HAL's `SupportInfo` as
/// it parcelled it (its words, its size first).
#[derive(Clone, Debug, PartialEq)]
struct ClientData {
    power_hal_version: i32,
    preferred_rate_nanos: i64,
    support_info: Vec<i32>,
}

impl WriteParcelable for ClientData {
    fn write_to(&self, p: &mut Parcel) {
        let start = p.position();
        p.write_i32(0);
        p.write_i32(self.power_hal_version);
        p.write_i32(MAX_GRAPHICS_PIPELINE_THREADS);
        p.write_i32(MAX_CPU_HEADROOM_THREADS);
        p.write_i64(self.preferred_rate_nanos);
        p.write_i32(1);
        for &w in &self.support_info {
            p.write_i32(w);
        }
        p.set_i32_at(start, (p.position() - start) as i32);
    }
}

/// A parcelable's words as `r` holds them: its size, then the rest.
fn parcelable_words(r: &mut Reader<'_>) -> ParcelResult<Vec<i32>> {
    let size = r.read_i32()?;
    if size < 4 || size % 4 != 0 {
        return Err(BAD_VALUE);
    }
    let mut words = vec![size];
    for _ in 1..size / 4 {
        words.push(r.read_i32()?);
    }
    Ok(words)
}

/// A call of the HAL: its reply's status, then what `read` reads.
fn hal_call<T>(hal: &Strong, code: u32, read: impl FnOnce(&mut Reader<'_>) -> ParcelResult<T>) -> Result<Result<T, Exception>, Exception> {
    let failed = |s| Exception::new(EX_ILLEGAL_STATE, format!("Could not contact PowerHAL! (#{code}: status {s})"));
    let mut data = Parcel::new();
    data.write_interface_token(POWER_DESCRIPTOR);
    let reply = hal.transact(code, &data, false).map_err(failed)?;
    let mut r = reply.reader();
    Ok(match r.read_exception().map_err(failed)? {
        Ok(()) => Ok(read(&mut r).map_err(failed)?),
        Err(e) => Err(e),
    })
}

pub struct HintManager {
    host: Arc<AppHost>,
    data: OnceLock<ClientData>,
}

impl HintManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        HintManager { host, data: OnceLock::new() }
    }

    /// What the constructor reads: `halGetHintSessionPreferredRate` (-1
    /// when the HAL has none) and `getSupportInfo`.
    fn client_data(&self) -> Result<ClientData, Exception> {
        if let Some(d) = self.data.get() {
            return Ok(d.clone());
        }
        let hal = super::display::check_service(&self.host.process, POWER).map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        let preferred_rate_nanos = hal_call(&hal, GET_HINT_SESSION_PREFERRED_RATE, |r| r.read_i64())?.unwrap_or(-1);
        let power_hal_version = hal_call(&hal, GET_INTERFACE_VERSION, |r| r.read_i32())??;
        if power_hal_version < 6 {
            return Err(self.host.unimplemented_named("performance_hint", "the support info of a Power HAL before version 6 (#1689)"));
        }
        let support_info = hal_call(&hal, GET_SUPPORT_INFO, |r| {
            if r.read_i32()? == 0 {
                return Err(BAD_VALUE);
            }
            parcelable_words(r)
        })??;
        Ok(self.data.get_or_init(|| ClientData { power_hal_version, preferred_rate_nanos, support_info }).clone())
    }
}

impl Service for HintManager {
    fn descriptor(&self) -> &str {
        hm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let result = match call.code {
            hm::REGISTER_CLIENT => {
                hm::RegisterClient::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.client_data().map(|d| hm::write_register_client_reply(&mut p, Some(&d)))
            }
            hm::GET_CLIENT_DATA => {
                hm::GetClientData::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.client_data().map(|d| hm::write_get_client_data_reply(&mut p, Some(&d)))
            }
            code => {
                let name = hm::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
                Err(self.host.unimplemented_named("performance_hint", &format!("{name} (#1689)")))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `HintManagerClientData.writeToParcel`: its size, the fields in
    /// order, and the support info as the HAL wrote it.
    #[test]
    fn client_data_parcels_as_the_original() {
        let d = ClientData { power_hal_version: 6, preferred_rate_nanos: 1_000_000, support_info: vec![12, 1, 0] };
        let mut p = Parcel::new();
        d.write_to(&mut p);
        let mut r = p.reader();
        assert_eq!(r.read_i32().unwrap(), 4 * 4 + 8 + 4 + 12);
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap(), r.read_i32().unwrap()), (6, 5, 5));
        assert_eq!(r.read_i64().unwrap(), 1_000_000);
        assert_eq!(r.read_i32().unwrap(), 1);
        let mut words = Reader::new(&p.data()[r.position()..], &[]);
        assert_eq!(parcelable_words(&mut words).unwrap(), [12, 1, 0]);
        assert_eq!(words.remaining(), 0);
    }
}
