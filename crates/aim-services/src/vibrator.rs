//! `vibrator_manager` (IVibratorManagerService) and
//! `external_vibrator_service` (IExternalVibratorService): the vibrator as
//! native services (ADR 0013), in place of SystemServer's
//! VibratorManagerService, which publishes both.
//!
//! A Mac has no vibration motor, and the derived image has no vibrator HAL
//! (`image/overlay.toml`), so the original runs without a vibrator: no
//! vibrator ids, no capabilities, no vibrator info, no state to listen to,
//! and nothing for a vibration to play on. This service is that original,
//! following `VibratorManagerService.java` at the pinned tag with no
//! `VibratorController`: its permission checks, in the original's order
//! and with its messages; the end a vendor session's callback is told
//! of; and the muted scale an external (audio-coupled) vibration gets.
//!
//! Not here: the `OP_VIBRATE` access the original records while a
//! vibration that its `VibrationSettings` let through plays on no vibrator
//! (#479), and its dump and shell command (#480).

use std::sync::Arc;

use aim_binder_host::local::{Call, LocalProcess, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_NULL_POINTER, Exception, Parcel, Reader, Result as ParcelResult,
    UNKNOWN_TRANSACTION,
};
use aim_service_aidl::{
    ReadParcelable, WriteParcelable, android_os_icancellationsignal as cancellation,
    android_os_iexternalvibratorservice as iev, android_os_ivibratormanagerservice as ivm,
    android_os_vibrator_ivibrationsessioncallback as session_callback,
};

use crate::clip::bundle;
use crate::system::System;

const VIBRATE: &str = "android.permission.VIBRATE";
const VIBRATE_ALWAYS_ON: &str = "android.permission.VIBRATE_ALWAYS_ON";
const VIBRATE_VENDOR_EFFECTS: &str = "android.permission.VIBRATE_VENDOR_EFFECTS";
const START_VIBRATION_SESSIONS: &str = "android.permission.START_VIBRATION_SESSIONS";
const ACCESS_VIBRATOR_STATE: &str = "android.permission.ACCESS_VIBRATOR_STATE";
const UPDATE_APP_OPS_STATS: &str = "android.permission.UPDATE_APP_OPS_STATS";
/// `IVibrationSession.STATUS_UNSUPPORTED`.
const STATUS_UNSUPPORTED: i32 = 3;
/// `ExternalVibrationScale.ScaleLevel.SCALE_MUTE`.
const SCALE_MUTE: i32 = -100;

type Result<T> = std::result::Result<T, Exception>;

fn bad_parcel(status: i32) -> Exception {
    Exception::illegal_argument(format!("bad parcel: status {status}"))
}

/// Who called.
struct Caller {
    pid: i32,
    uid: i32,
}

pub struct VibratorManagerService {
    system: Arc<System>,
    process: Arc<LocalProcess>,
    /// The cancellation signal handed out for a vendor session, which
    /// ends as it starts: cancelling it changes nothing.
    ended_session: Binder,
}

impl VibratorManagerService {
    pub fn new(process: Arc<LocalProcess>, system: Arc<System>) -> Arc<Self> {
        let ended_session = process.add_service(Arc::new(EndedSession));
        Arc::new(Self {
            system,
            process,
            ended_session,
        })
    }

    /// `Context.enforceCallingOrSelfPermission(permission, message)`.
    fn enforce_calling(&self, caller: &Caller, permission: &str, message: &str) -> Result<()> {
        if self
            .system
            .check_permission(permission, caller.pid, caller.uid)?
        {
            return Ok(());
        }
        Err(Exception::security(format!(
            "{message}: Neither user {} nor current process has {permission}.",
            caller.uid
        )))
    }

    /// `PermissionEnforcer.enforcePermissionAllOf`, what
    /// `@EnforcePermission(allOf = ...)` generates.
    fn enforce_all_of(&self, caller: &Caller, permissions: &[&str]) -> Result<()> {
        for permission in permissions {
            if !self
                .system
                .check_permission(permission, caller.pid, caller.uid)?
            {
                return Err(Exception::security(format!(
                    "Access denied, requires: allOf={{{}}}",
                    permissions.join(", ")
                )));
            }
        }
        Ok(())
    }

    /// `enforceUpdateAppOpsStatsPermission`: acting for another uid needs
    /// UPDATE_APP_OPS_STATS (`Context.enforcePermission`).
    fn enforce_update_app_ops_stats(&self, caller: &Caller, uid: i32) -> Result<()> {
        if uid == caller.uid
            || self
                .system
                .check_permission(UPDATE_APP_OPS_STATS, caller.pid, caller.uid)?
        {
            return Ok(());
        }
        Err(Exception::security(format!(
            "uid {} does not have {UPDATE_APP_OPS_STATS}.",
            caller.uid
        )))
    }

    fn dispatch(&self, call: &mut Call<'_>) -> Result<Option<Parcel>> {
        let caller = Caller {
            pid: call.sender_pid,
            uid: call.sender_euid as i32,
        };
        let mut reply = Parcel::new();
        let r = &mut call.data;
        match call.code {
            ivm::GET_VIBRATOR_IDS => {
                ivm::GetVibratorIds::read(r).map_err(bad_parcel)?;
                ivm::write_get_vibrator_ids_reply(&mut reply, &Some(Vec::new()));
            }
            ivm::GET_CAPABILITIES => {
                ivm::GetCapabilities::read(r).map_err(bad_parcel)?;
                ivm::write_get_capabilities_reply(&mut reply, 0);
            }
            ivm::GET_VIBRATOR_INFO => {
                ivm::GetVibratorInfo::read(r).map_err(bad_parcel)?;
                ivm::write_get_vibrator_info_reply::<VibratorInfo>(&mut reply, None);
            }
            ivm::IS_VIBRATING
            | ivm::REGISTER_VIBRATOR_STATE_LISTENER
            | ivm::UNREGISTER_VIBRATOR_STATE_LISTENER => {
                match call.code {
                    ivm::IS_VIBRATING => ivm::IsVibrating::read(r).map(drop),
                    ivm::REGISTER_VIBRATOR_STATE_LISTENER => {
                        ivm::RegisterVibratorStateListener::read(r).map(drop)
                    }
                    _ => ivm::UnregisterVibratorStateListener::read(r).map(drop),
                }
                .map_err(bad_parcel)?;
                self.system
                    .enforce_permission(ACCESS_VIBRATOR_STATE, caller.pid, caller.uid)?;
                // No vibrator has the id, so none vibrates or takes a listener.
                reply.write_no_exception();
                reply.write_bool(false);
            }
            ivm::SET_ALWAYS_ON_EFFECT => {
                let a = ivm::SetAlwaysOnEffect::<CombinedVibration, VibrationAttributes>::read(r)
                    .map_err(bad_parcel)?;
                self.enforce_calling(&caller, VIBRATE_ALWAYS_ON, "setAlwaysOnEffect")?;
                // Removing an effect succeeds; setting one fails for want of
                // a vibrator with always-on control.
                ivm::write_set_always_on_effect_reply(&mut reply, a.vibration.is_none());
            }
            ivm::VIBRATE => {
                let a = ivm::Vibrate::<CombinedVibration, VibrationAttributes>::read(r)
                    .map_err(bad_parcel)?;
                self.enforce_calling(&caller, VIBRATE, "vibrate")?;
                // Without a token the vibration is ignored before the uid is
                // checked; with one, it has no vibrator to play on.
                if a.token.is_some() {
                    self.enforce_update_app_ops_stats(&caller, a.uid)?;
                }
                ivm::write_vibrate_reply(&mut reply);
            }
            ivm::CANCEL_VIBRATE => {
                ivm::CancelVibrate::read(r).map_err(bad_parcel)?;
                self.enforce_calling(&caller, VIBRATE, "cancelVibrate")?;
                // No vibration plays, so none matches.
                ivm::write_cancel_vibrate_reply(&mut reply);
            }
            ivm::PERFORM_HAPTIC_FEEDBACK => {
                // One-way: a vibration without a permission check, on no
                // vibrator.
                ivm::PerformHapticFeedback::read(r).map_err(bad_parcel)?;
            }
            ivm::PERFORM_HAPTIC_FEEDBACK_FOR_INPUT_DEVICE => {
                ivm::PerformHapticFeedbackForInputDevice::read(r).map_err(bad_parcel)?;
            }
            ivm::START_VENDOR_VIBRATION_SESSION => {
                let a = ivm::StartVendorVibrationSession::<VibrationAttributes>::read(r)
                    .map_err(bad_parcel)?;
                self.enforce_all_of(
                    &caller,
                    &[VIBRATE, VIBRATE_VENDOR_EFFECTS, START_VIBRATION_SESSIONS],
                )?;
                let session = match a.callback {
                    None => None,
                    Some(callback) => {
                        self.enforce_update_app_ops_stats(&caller, a.uid)?;
                        // Without CAP_START_SESSIONS the session is
                        // unsupported: it ends without starting.
                        self.session_finished(callback, STATUS_UNSUPPORTED);
                        Some(self.ended_session)
                    }
                };
                ivm::write_start_vendor_vibration_session_reply(&mut reply, session);
            }
            _ => return Ok(None),
        }
        Ok(Some(reply))
    }

    /// `IVibrationSessionCallback.onFinished(status)`, one-way.
    fn session_finished(&self, callback: Binder, status: i32) {
        let Binder::Handle(handle) = callback else {
            return;
        };
        let mut data = Parcel::new();
        session_callback::OnFinished { status }.write(&mut data);
        let _ = self
            .process
            .transact(handle, session_callback::ON_FINISHED, &data, true);
    }
}

impl Service for VibratorManagerService {
    fn descriptor(&self) -> &str {
        ivm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        answer(self.dispatch(call))
    }
}

/// A call's reply, or its exception.
fn answer(result: Result<Option<Parcel>>) -> Reply {
    match result {
        Ok(Some(reply)) => Ok(reply),
        Ok(None) => Err(UNKNOWN_TRANSACTION),
        Err(exception) => {
            let mut reply = Parcel::new();
            reply.write_exception(&exception);
            Ok(reply)
        }
    }
}

/// The `ICancellationSignal` of a vendor session that already ended.
struct EndedSession;

impl Service for EndedSession {
    fn descriptor(&self) -> &str {
        cancellation::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            cancellation::CANCEL => Ok(Parcel::new()),
            _ => Err(UNKNOWN_TRANSACTION),
        }
    }
}

/// `external_vibrator_service`: audioserver asks it for the scale of the
/// haptic channels it plays. Without a vibrator that has external
/// control, every external vibration is muted and ends at once.
pub struct ExternalVibratorService;

impl ExternalVibratorService {
    fn dispatch(call: &mut Call<'_>) -> Result<Option<Parcel>> {
        let mut reply = Parcel::new();
        let r = &mut call.data;
        match call.code {
            iev::ON_EXTERNAL_VIBRATION_START => {
                let a = iev::OnExternalVibrationStart::<ExternalVibration>::read(r)
                    .map_err(bad_parcel)?;
                if a.vib.is_none() {
                    return Err(Exception::new(EX_NULL_POINTER, "vib"));
                }
                iev::write_on_external_vibration_start_reply(
                    &mut reply,
                    Some(&ExternalVibrationScale::MUTED),
                );
            }
            iev::ON_EXTERNAL_VIBRATION_STOP => {
                // No external vibration is ever current.
                iev::OnExternalVibrationStop::<ExternalVibration>::read(r).map_err(bad_parcel)?;
                iev::write_on_external_vibration_stop_reply(&mut reply);
            }
            _ => return Ok(None),
        }
        Ok(Some(reply))
    }
}

impl Service for ExternalVibratorService {
    fn descriptor(&self) -> &str {
        iev::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        answer(Self::dispatch(call))
    }
}

/// `VibratorInfo`, of which there is none to write: no vibrator has an id.
enum VibratorInfo {}

impl WriteParcelable for VibratorInfo {
    fn write_to(&self, _: &mut Parcel) {
        match *self {}
    }
}

/// `ExternalVibrationScale`, an AIDL structured parcelable.
struct ExternalVibrationScale {
    scale_level: i32,
    scale_factor: f32,
    adaptive_haptics_scale: f32,
}

impl ExternalVibrationScale {
    /// `ExternalVibrationSession.muteScale` with `haptics_scale_v2_enabled`
    /// off in the image: the level muted, the factor left undefined.
    const MUTED: Self = Self {
        scale_level: SCALE_MUTE,
        scale_factor: -1.0,
        adaptive_haptics_scale: 1.0,
    };
}

impl WriteParcelable for ExternalVibrationScale {
    fn write_to(&self, p: &mut Parcel) {
        // Its size first, including the size itself.
        let start = p.position();
        p.write_i32(0);
        p.write_i32(self.scale_level);
        p.write_f32(self.scale_factor);
        p.write_f32(self.adaptive_haptics_scale);
        p.set_i32_at(start, (p.position() - start) as i32);
    }
}

/// `ExternalVibration(Parcel)`, read to its end.
struct ExternalVibration;

impl ReadParcelable for ExternalVibration {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        r.read_i32()?; // uid
        r.read_string16()?; // package
        for _ in 0..4 {
            r.read_i32()?; // usage, content type, capture preset, flags
        }
        r.read_binder()?; // controller
        r.read_binder()?; // token
        Ok(Self)
    }
}

/// `VibrationAttributes(Parcel)`: usage, original audio usage and flags.
/// With no vibrator they decide nothing.
struct VibrationAttributes;

impl ReadParcelable for VibrationAttributes {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        for _ in 0..3 {
            r.read_i32()?;
        }
        Ok(Self)
    }
}

/// `CombinedVibration.CREATOR`, read to its end: with no vibrator the
/// effects decide nothing, and only whether one was sent matters.
struct CombinedVibration;

/// `CombinedVibration`'s and `VibrationEffect`'s parcel tokens.
const MONO: i32 = 1;
const STEREO: i32 = 2;
const SEQUENTIAL: i32 = 3;
const COMPOSED: i32 = 1;
const VENDOR_EFFECT: i32 = 2;
/// `Parcel.VAL_NULL` and `VAL_PARCELABLE` (length-prefixed).
const VAL_NULL: i32 = -1;
const VAL_PARCELABLE: i32 = 4;

impl ReadParcelable for CombinedVibration {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        // A sequential combination's parts follow its size, each after its
        // delay; counted rather than recursed into, however deep.
        let mut pending = 1_usize;
        let mut first = true;
        while pending > 0 {
            pending -= 1;
            if !std::mem::take(&mut first) {
                r.read_i32()?; // delay
            }
            match r.read_i32()? {
                MONO => vibration_effect(r)?,
                STEREO => {
                    for _ in 0..count(r)? {
                        r.read_i32()?; // vibrator id
                        vibration_effect(r)?;
                    }
                }
                SEQUENTIAL => pending += count(r)?,
                _ => return Err(BAD_VALUE),
            }
        }
        Ok(Self)
    }
}

/// A collection's size, which Java refuses negative.
fn count(r: &mut Reader<'_>) -> ParcelResult<usize> {
    usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)
}

/// `VibrationEffect.CREATOR`.
fn vibration_effect(r: &mut Reader<'_>) -> ParcelResult<()> {
    match r.read_i32()? {
        COMPOSED => {
            // `readArrayList` of segments, each a length-prefixed value.
            for _ in 0..count(r)? {
                match r.read_i32()? {
                    VAL_NULL => {}
                    VAL_PARCELABLE => {
                        let length = usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)?;
                        r.skip(length)?;
                    }
                    _ => return Err(BAD_VALUE),
                }
            }
            r.read_i32()?; // repeat index
        }
        VENDOR_EFFECT => {
            // The vendor data (a PersistableBundle, not null), strength,
            // scale and adaptive scale.
            if !bundle(r)? {
                return Err(BAD_VALUE);
            }
            r.read_i32()?;
            r.read_f32()?;
            r.read_f32()?;
        }
        _ => return Err(BAD_VALUE),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(p: &Parcel) -> ParcelResult<CombinedVibration> {
        CombinedVibration::read_from(&mut Reader::new(p.data(), p.objects()))
    }

    /// `VibrationEffect.createOneShot(100, 255)`: one StepSegment.
    fn one_shot(p: &mut Parcel) {
        p.write_i32(COMPOSED);
        p.write_i32(1);
        p.write_i32(VAL_PARCELABLE);
        let mut segment = Parcel::new();
        segment.write_string16(Some("android.os.vibrator.StepSegment"));
        segment.write_i32(3); // parcel token
        segment.write_f32(1.0);
        segment.write_f32(0.0);
        segment.write_i32(100);
        p.write_i32(segment.data().len() as i32);
        p.write_raw(segment.data(), &[]);
        p.write_i32(-1); // repeat index
    }

    #[test]
    fn reads_combinations_to_their_end() {
        let mut p = Parcel::new();
        p.write_i32(SEQUENTIAL);
        p.write_i32(2);
        p.write_i32(0); // delay
        p.write_i32(MONO);
        one_shot(&mut p);
        p.write_i32(50); // delay
        p.write_i32(STEREO);
        p.write_i32(1);
        p.write_i32(7); // vibrator id
        one_shot(&mut p);
        p.write_i32(0x5a5a);
        let mut r = Reader::new(p.data(), p.objects());
        CombinedVibration::read_from(&mut r).unwrap();
        assert_eq!(r.read_i32().unwrap(), 0x5a5a);
    }

    #[test]
    fn reads_a_vendor_effect() {
        let mut p = Parcel::new();
        p.write_i32(MONO);
        p.write_i32(VENDOR_EFFECT);
        p.write_i32(8); // bundle length
        p.write_i32(0x4C44_4E42); // BUNDLE_MAGIC
        p.write_i32(1); // one entry, ...
        p.write_i32(0); // ... cut short: its bytes are skipped whole
        p.write_bool(false); // has an intent
        p.write_i32(2); // strength
        p.write_f32(1.0);
        p.write_f32(1.0);
        p.write_i32(0x5a5a);
        let mut r = Reader::new(p.data(), p.objects());
        CombinedVibration::read_from(&mut r).unwrap();
        assert_eq!(r.read_i32().unwrap(), 0x5a5a);
    }

    #[test]
    fn refuses_what_java_refuses() {
        for bad in [[9, 0], [STEREO, -1]] {
            let mut p = Parcel::new();
            bad.iter().for_each(|&v| p.write_i32(v));
            assert!(read(&p).is_err());
        }
        let mut p = Parcel::new();
        p.write_i32(MONO);
        p.write_i32(VENDOR_EFFECT);
        p.write_i32(-1); // no vendor data
        assert!(read(&p).is_err());
    }

    #[test]
    fn writes_the_structured_scale() {
        let mut p = Parcel::new();
        ExternalVibrationScale::MUTED.write_to(&mut p);
        let mut r = Reader::new(p.data(), p.objects());
        assert_eq!(r.read_i32().unwrap(), 16);
        assert_eq!(r.read_i32().unwrap(), SCALE_MUTE);
        assert_eq!(r.read_f32().unwrap(), -1.0);
        assert_eq!(r.read_f32().unwrap(), 1.0);
    }
}
