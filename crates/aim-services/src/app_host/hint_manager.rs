//! `performance_hint`: HintManagerService for the app host
//! (`android-16.0.0_r1`), the client data every process's
//! `APerformanceHintManager` registers for (`registerClient`,
//! `getClientData`): the Power HAL's version, preferred rate and support
//! info, as the service reads them at its start; and the FMQ session
//! channel as far as a HAL without a separate event flag goes (none: the
//! service closes the HAL's channel and falls back for good). Hint
//! sessions, a HAL's channel with an event flag, headroom and a Power HAL
//! before version 6 are not implemented (#1689).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_os_ihintmanager as hm;

use super::{AppHost, reply};

/// The Power HAL (`IPower/default`), its descriptor and the calls the
/// service makes.
const POWER: &str = "android.hardware.power.IPower/default";
const POWER_DESCRIPTOR: &str = "android.hardware.power.IPower";
const GET_HINT_SESSION_PREFERRED_RATE: u32 = 6;
const GET_SESSION_CHANNEL: u32 = 8;
const CLOSE_SESSION_CHANNEL: u32 = 9;
const GET_SUPPORT_INFO: u32 = 10;
/// The aconfig flag of the FMQ channel (`Flags.adpfUseFmqChannel`).
const ADPF_USE_FMQ_CHANNEL: &str = "android.os.adpf_use_fmq_channel";
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

/// Whether a HAL's `ChannelConfig` has an `eventFlagDescriptor`: past its
/// size and its `channelDescriptor` (present, then sized), whether the
/// nullable one is there.
fn has_event_flag(r: &mut Reader<'_>) -> ParcelResult<bool> {
    if r.read_i32()? == 0 {
        return Err(BAD_VALUE);
    }
    r.read_i32()?;
    if r.read_i32()? == 0 {
        return Err(BAD_VALUE);
    }
    let start = r.position();
    let size = r.read_i32()?;
    if size < 4 {
        return Err(BAD_VALUE);
    }
    r.set_position(start + size as usize);
    Ok(r.read_i32()? != 0)
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
    hal_call_with(hal, code, |_| {}, read)
}

fn hal_call_with<T>(
    hal: &Strong,
    code: u32,
    write: impl FnOnce(&mut Parcel),
    read: impl FnOnce(&mut Reader<'_>) -> ParcelResult<T>,
) -> Result<Result<T, Exception>, Exception> {
    let failed = |s| Exception::new(EX_ILLEGAL_STATE, format!("Could not contact PowerHAL! (#{code}: status {s})"));
    let mut data = Parcel::new();
    data.write_interface_token(POWER_DESCRIPTOR);
    write(&mut data);
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
    /// `mFMQUsesIntegratedEventFlag`: the HAL's channel had no event flag.
    integrated_event_flag: AtomicBool,
}

impl HintManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        HintManager { host, data: OnceLock::new(), integrated_event_flag: AtomicBool::new(false) }
    }

    /// Whether the FMQ channel is in use: a HAL from version 5 and the
    /// flag on.
    fn uses_channel(&self) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        let flag = state.system.flags.iter().any(|(n, on)| n == ADPF_USE_FMQ_CHANNEL && *on);
        Ok(self.client_data()?.power_hal_version >= 5 && flag)
    }

    /// `getSessionChannel` of the caller's process (`tgid`) and `uid`:
    /// none without the channel or once a HAL showed it has no event flag;
    /// else the HAL's channel is opened, and closed again when it has none.
    fn session_channel(&self, tgid: i32, uid: i32, token: Option<Binder>) -> Result<(), Exception> {
        if !self.uses_channel()? || self.integrated_event_flag.load(Ordering::Acquire) {
            return Ok(());
        }
        if token.is_none() {
            return Err(Exception::new(EX_NULL_POINTER, "token"));
        }
        let hal = super::display::check_service(&self.host.process, POWER).map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        let ids = |p: &mut Parcel| {
            p.write_i32(tgid);
            p.write_i32(uid);
        };
        let event_flag = hal_call_with(&hal, GET_SESSION_CHANNEL, ids, has_event_flag)??;
        // `closeSessionChannel`, which closes the HAL's.
        let mut data = Parcel::new();
        data.write_interface_token(POWER_DESCRIPTOR);
        ids(&mut data);
        hal.transact(CLOSE_SESSION_CHANNEL, &data, true).map_err(|s| Exception::new(EX_ILLEGAL_STATE, format!("closeSessionChannel: status {s}")))?;
        if event_flag {
            return Err(self.host.unimplemented_named("performance_hint", "a session channel with an event flag (#1689)"));
        }
        self.integrated_event_flag.store(true, Ordering::Release);
        Ok(())
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
            hm::GET_SESSION_CHANNEL => {
                let a = hm::GetSessionChannel::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.session_channel(call.sender_pid, call.sender_euid as i32, a.token).map(|()| hm::write_get_session_channel_reply::<ClientData>(&mut p, None))
            }
            // A channel is never kept open, so there is none to close
            // (`removeChannelItem`).
            hm::CLOSE_SESSION_CHANNEL => Ok(()),
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

    /// A HAL's `ChannelConfig` reply: present, its size, its present
    /// `channelDescriptor` (skipped by its size), then whether an event
    /// flag descriptor follows.
    #[test]
    fn channel_config_tells_whether_it_has_an_event_flag() {
        for flag in [0, 1] {
            let mut p = Parcel::new();
            p.write_i32(1);
            p.write_i32(36);
            p.write_i32(1);
            for w in [12, 7, 9] {
                p.write_i32(w);
            }
            p.write_i32(flag);
            let mut r = p.reader();
            assert_eq!(has_event_flag(&mut r).unwrap(), flag == 1);
        }
    }
}
