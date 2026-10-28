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
//! Accepted vibrations start and finish OP_VIBRATE through the AppOps
//! owner, even without a motor. Dump and shell commands remain #480.
//! Policy/effect rules follow android-16.0.0_r1 (AOSP, Apache-2.0).

#[path = "vibrator_policy.rs"]
pub(crate) mod policy;

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
    com_android_internal_app_iappopsservice as appops,
};

use crate::{package::restrictions::persistable::Bundle, system::System};

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
    /// ends as it starts: cancelling it changes nothing. Also the
    /// process-owned AppOps client token; app tokens never own the interval.
    ended_session: Binder,
    operations: std::sync::Mutex<()>,
}

impl VibratorManagerService {
    pub fn new(process: Arc<LocalProcess>, system: Arc<System>) -> Arc<Self> {
        let ended_session = process.add_service(Arc::new(EndedSession));
        Arc::new(Self {
            system,
            process: process.clone(),
            ended_session,
            operations: std::sync::Mutex::new(()),
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
                let effect = a.vibration.as_ref();
                let attrs =
                    self.fixup_attributes(&caller, a.attributes.unwrap_or_default(), effect)?;
                self.enforce_calling(&caller, VIBRATE, "vibrate")?;
                if a.token.is_some() {
                    self.enforce_update_app_ops_stats(&caller, a.uid)?;
                    if let Some(effect) = effect.filter(|e| e.valid) {
                        if !effect.vendor
                            || self.system.check_permission(
                                VIBRATE_VENDOR_EFFECTS,
                                caller.pid,
                                caller.uid,
                            )?
                        {
                            self.record(a.uid, a.device_id, a.op_pkg.as_deref(), &attrs)?;
                        }
                    }
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
                let a = ivm::PerformHapticFeedback::read(r).map_err(bad_parcel)?;
                self.perform_haptic(
                    &caller,
                    a.uid,
                    a.device_id,
                    a.op_pkg.as_deref(),
                    a.constant,
                    0,
                    a.flags,
                    a.priv_flags,
                )?;
            }
            ivm::PERFORM_HAPTIC_FEEDBACK_FOR_INPUT_DEVICE => {
                let a = ivm::PerformHapticFeedbackForInputDevice::read(r).map_err(bad_parcel)?;
                self.perform_haptic(
                    &caller,
                    a.uid,
                    a.device_id,
                    a.op_pkg.as_deref(),
                    a.constant,
                    a.input_source,
                    a.flags,
                    a.priv_flags,
                )?;
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

    fn fixup_attributes(
        &self,
        caller: &Caller,
        mut attrs: VibrationAttributes,
        effect: Option<&CombinedVibration>,
    ) -> Result<VibrationAttributes> {
        if attrs.usage == 0 && effect.is_some_and(|e| e.haptic) {
            attrs.usage = policy::TOUCH;
        }
        if attrs.flags & 3 != 0 {
            let mut privileged = false;
            for permission in [
                "android.permission.WRITE_SECURE_SETTINGS",
                "android.permission.MODIFY_PHONE_STATE",
                "android.permission.MODIFY_AUDIO_ROUTING",
            ] {
                if self
                    .system
                    .check_permission(permission, caller.pid, caller.uid)?
                {
                    privileged = true;
                    break;
                }
            }
            if !privileged {
                attrs.flags &= !3;
            }
        }
        Ok(attrs)
    }

    #[allow(clippy::too_many_arguments)]
    fn perform_haptic(
        &self,
        caller: &Caller,
        uid: i32,
        device_id: i32,
        package: Option<&str>,
        constant: i32,
        input_source: i32,
        flags: i32,
        priv_flags: i32,
    ) -> Result<()> {
        if matches!(constant, 10004 | 10005)
            && !self.system.check_permission(
                "android.permission.VIBRATE_SYSTEM_CONSTANTS",
                caller.pid,
                caller.uid,
            )?
        {
            return Ok(());
        }
        if let Some(attrs) =
            self.system
                .haptic_attributes(constant, input_source, flags, priv_flags)?
        {
            // vibrateWithoutPermissionCheck deliberately preserves provider flags.
            self.enforce_update_app_ops_stats(caller, uid)?;
            self.record(uid, device_id, package, &attrs)?;
        }
        Ok(())
    }

    /// The no-vibrator conductor releases immediately. Serialize its complete
    /// start/release interval as VibratorManagerService does under mLock so
    /// concurrent requests cannot merge into one active AppOps interval.
    fn record(
        &self,
        uid: i32,
        device_id: i32,
        package: Option<&str>,
        attrs: &VibrationAttributes,
    ) -> Result<()> {
        let _operation = self.operations.lock().unwrap();
        if self.system.vibration_ignored(uid, device_id, attrs)? {
            return Ok(());
        }
        let check = appops::CheckAudioOperation {
            code: 3,
            usage: attrs.audio_usage(),
            uid,
            package_name: package.map(str::to_owned),
        };
        let mode = self.system.call(
            "appops",
            appops::CHECK_AUDIO_OPERATION,
            |p| check.write(p),
            appops::read_check_audio_operation_reply,
        )?;
        if !attrs.allows(mode) {
            return Ok(());
        }
        let _identity = aim_binder_host::local::clear_calling_identity();
        let start = appops::StartOperation {
            client_id: Some(self.ended_session),
            code: 3,
            uid,
            package_name: package.map(str::to_owned),
            attribution_tag: None,
            start_if_mode_default: false,
            should_collect_async_noted_op: false,
            message: None,
            should_collect_message: false,
            attribution_flags: 0,
            attribution_chain_id: -1,
        };
        let started = self
            .system
            .call(
                "appops",
                appops::START_OPERATION,
                |p| start.write(p),
                appops::read_start_operation_reply::<crate::system::NotedOp>,
            )?
            .ok_or_else(|| bad_parcel(BAD_VALUE))?;
        if attrs.allows(started.mode) {
            let finish = appops::FinishOperation {
                client_id: Some(self.ended_session),
                code: 3,
                uid,
                package_name: package.map(str::to_owned),
                attribution_tag: None,
            };
            self.system.call(
                "appops",
                appops::FINISH_OPERATION,
                |p| finish.write(p),
                appops::read_finish_operation_reply,
            )?;
        }
        Ok(())
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
        let result = self.dispatch(call);
        if call.flags & aim_binder_driver::uapi::TF_ONE_WAY != 0 {
            if let Err(error) = &result {
                eprintln!("vibrator: one-way call {}: {}", call.code, error.message);
            }
        }
        answer(result)
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

/// VibrationAttributes(Parcel); flags are masked by its constructor.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct VibrationAttributes {
    pub usage: i32,
    pub audio_usage: i32,
    pub flags: i32,
}
impl VibrationAttributes {
    fn audio_usage(&self) -> i32 {
        if self.audio_usage != 0 {
            return self.audio_usage;
        }
        match self.usage {
            policy::NOTIFICATION => 5,
            policy::COMMUNICATION => 2,
            policy::RINGTONE => 6,
            policy::TOUCH | policy::IME => 13,
            policy::ALARM => 4,
            policy::ACCESSIBILITY => 11,
            19 => 1,
            _ => 0,
        }
    }
    fn allows(&self, mode: i32) -> bool {
        mode == 0 || mode == 1 && self.flags & 1 != 0
    }
}
impl ReadParcelable for VibrationAttributes {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(Self {
            usage: r.read_i32()?,
            audio_usage: r.read_i32()?,
            flags: r.read_i32()? & 15,
        })
    }
}
impl WriteParcelable for VibrationAttributes {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.usage);
        p.write_i32(self.audio_usage);
        p.write_i32(self.flags);
    }
}

/// HapticFeedbackVibrationProvider with EMPTY_VIBRATOR_INFO: predefined
/// fallbacks exist even though no vibrator can play them.
pub(crate) fn haptic_attributes(
    constant: i32,
    input_source: i32,
    flags: i32,
    priv_flags: i32,
) -> Option<VibrationAttributes> {
    let usage = match constant {
        14 | 15 => policy::PHYSICAL,
        10002 | 10003 => policy::HARDWARE,
        18..=20 if input_source == 0 || input_source == 0x00400000 => policy::HARDWARE,
        3 | 7 if priv_flags & 1 != 0 => policy::IME,
        10004 | 10005 => policy::COMMUNICATION,
        0 | 1 | 3..=9 | 11..=13 | 16..=27 | 10001 => policy::TOUCH,
        _ => return None,
    };
    Some(VibrationAttributes {
        usage,
        audio_usage: 0,
        flags: if flags & 2 != 0 { 2 } else { 0 } | if matches!(constant, 18..=20) { 1 } else { 0 },
    })
}

const MONO: i32 = 1;
const STEREO: i32 = 2;
const SEQUENTIAL: i32 = 3;
const COMPOSED: i32 = 1;
const VENDOR_EFFECT: i32 = 2;
const VAL_NULL: i32 = -1;
const VAL_PARCELABLE: i32 = 4;

#[derive(Debug)]
struct CombinedVibration {
    pub valid: bool,
    pub haptic: bool,
    pub vendor: bool,
}

fn count(r: &mut Reader<'_>) -> ParcelResult<usize> {
    let n = usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)?;
    if n > r.remaining() / 4 {
        return Err(BAD_VALUE);
    }
    Ok(n)
}

impl ReadParcelable for CombinedVibration {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let mut result = Self {
            valid: true,
            haptic: true,
            vendor: false,
        };
        // Remaining children of each sequential parent. Iteration also reads
        // invalid nested sequences to their end without recursing on IPC data.
        let mut parents = vec![1_usize];
        while let Some(remaining) = parents.last_mut() {
            if *remaining == 0 {
                parents.pop();
                continue;
            }
            *remaining -= 1;
            if parents.len() > 1 {
                result.valid &= r.read_i32()? >= 0;
            }
            match r.read_i32()? {
                MONO => {
                    let effect = vibration_effect(r)?;
                    result.valid &= effect.valid;
                    result.haptic &= effect.haptic;
                    result.vendor |= effect.vendor;
                }
                STEREO => {
                    let n = count(r)?;
                    result.valid &= n > 0;
                    // SparseArray.put replaces duplicate ids, as Java does.
                    let mut effects = std::collections::BTreeMap::new();
                    for _ in 0..n {
                        effects.insert(r.read_i32()?, vibration_effect(r)?);
                    }
                    for e in effects.values() {
                        result.valid &= e.valid;
                        result.haptic &= e.haptic;
                        result.vendor |= e.vendor;
                    }
                }
                SEQUENTIAL => {
                    let n = count(r)?;
                    result.valid &= n > 0 && parents.len() == 1;
                    result.haptic &= n <= 3;
                    parents.push(n);
                }
                _ => return Err(BAD_VALUE),
            }
        }
        Ok(result)
    }
}

fn vibration_effect(r: &mut Reader<'_>) -> ParcelResult<CombinedVibration> {
    let mut result = CombinedVibration {
        valid: true,
        haptic: true,
        vendor: false,
    };
    match r.read_i32()? {
        COMPOSED => {
            let n = count(r)?;
            let mut nonzero = false;
            let mut duration = 0_i64;
            result.haptic = n <= 3;
            for _ in 0..n {
                match r.read_i32()? {
                    VAL_NULL => {
                        result.valid = false;
                    }
                    VAL_PARCELABLE => {
                        let len = usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)?;
                        let start = r.position();
                        r.skip(len)?;
                        let (bytes, objects) = r.since(start);
                        let mut segment = Reader::new(bytes, &objects);
                        let (valid, haptic, time) = segment_info(&mut segment)?;
                        result.valid &= valid;
                        result.haptic &= haptic;
                        nonzero |= time != 0;
                        duration = duration.saturating_add(time.max(0));
                    }
                    _ => return Err(BAD_VALUE),
                }
            }
            let repeat = r.read_i32()?;
            result.valid &= nonzero && (repeat == -1 || (0..n as i32).contains(&repeat));
            result.haptic &= repeat < 0 && duration <= 1000;
        }
        VENDOR_EFFECT => {
            let data = Bundle::read_from(r)?;
            let nonempty = !data.entries.is_empty();
            r.read_i32()?;
            r.read_f32()?;
            r.read_f32()?;
            result.valid = nonempty;
            result.haptic = false;
            result.vendor = true;
        }
        _ => return Err(BAD_VALUE),
    }
    Ok(result)
}

fn unit(v: f32) -> bool {
    (0.0..=1.0).contains(&v)
}
fn frequency(v: f32) -> bool {
    v.is_finite() && v >= 0.0
}

fn segment_info(r: &mut Reader<'_>) -> ParcelResult<(bool, bool, i64)> {
    let name = r.read_string16()?.ok_or(BAD_VALUE)?;
    r.read_i32()?; // The class CREATOR skips the type token.
    match name.as_str() {
        "android.os.vibrator.StepSegment" => {
            let amplitude = r.read_f32()?;
            let freq = r.read_f32()?;
            let time = r.read_i32()?;
            Ok((
                (unit(amplitude) || amplitude == -1.0 && freq.to_bits() == 0)
                    && frequency(freq)
                    && time >= 0,
                true,
                time as i64,
            ))
        }
        "android.os.vibrator.PrimitiveSegment" => {
            let id = r.read_i32()?;
            let scale = r.read_f32()?;
            let delay = r.read_i32()?;
            let delay_type = r.read_i32()?;
            Ok((
                (0..=8).contains(&id) && unit(scale) && delay >= 0 && matches!(delay_type, 0 | 1),
                true,
                -1,
            ))
        }
        "android.os.vibrator.PrebakedSegment" => {
            let id = r.read_i32()?;
            r.read_i32()?;
            let strength = r.read_i32()?;
            Ok((
                (0..=21).contains(&id) && (0..=2).contains(&strength),
                matches!(id, 0..=5 | 21),
                -1,
            ))
        }
        "android.os.vibrator.RampSegment"
        | "android.os.vibrator.PwleSegment"
        | "android.os.vibrator.BasicPwleSegment" => {
            let (a, b, c, d) = (r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?);
            let ramp = name.ends_with(".RampSegment");
            let time = if ramp {
                r.read_i32()? as i64
            } else {
                r.read_i64()?
            };
            let frequencies = if ramp {
                frequency(c) && frequency(d)
            } else if name.ends_with(".BasicPwleSegment") {
                unit(c) && unit(d)
            } else {
                c > 0.0 && d > 0.0
            };
            Ok((
                unit(a) && unit(b) && frequencies && if ramp { time >= 0 } else { time > 0 },
                true,
                time,
            ))
        }
        _ => Err(BAD_VALUE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(p: &Parcel) -> ParcelResult<CombinedVibration> {
        CombinedVibration::read_from(&mut Reader::new(p.data(), p.objects()))
    }

    /// `VibrationEffect.createOneShot(100, 255)`: one StepSegment.
    fn one_shot(p: &mut Parcel, duration: i32, amplitude: f32, frequency: f32, repeat: i32) {
        p.write_i32(COMPOSED);
        p.write_i32(1);
        p.write_i32(VAL_PARCELABLE);
        let mut segment = Parcel::new();
        segment.write_string16(Some("android.os.vibrator.StepSegment"));
        segment.write_i32(3); // parcel token
        segment.write_f32(amplitude);
        segment.write_f32(frequency);
        segment.write_i32(duration);
        p.write_i32(segment.data().len() as i32);
        p.write_raw(segment.data(), &[]);
        p.write_i32(repeat); // repeat index
    }

    #[test]
    fn reads_combinations_to_their_end() {
        let mut p = Parcel::new();
        p.write_i32(SEQUENTIAL);
        p.write_i32(2);
        p.write_i32(0); // delay
        p.write_i32(MONO);
        one_shot(&mut p, 100, 1.0, 0.0, -1);
        p.write_i32(50); // delay
        p.write_i32(STEREO);
        p.write_i32(1);
        p.write_i32(7); // vibrator id
        one_shot(&mut p, 100, 1.0, 0.0, -1);
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
        let data = crate::package::restrictions::persistable::Bundle {
            entries: vec![(
                Some("effect".into()),
                crate::package::restrictions::persistable::Value::Int(7),
            )],
        }
        .parcel()
        .unwrap();
        p.write_raw(data.data(), &[]);
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
    #[test]
    fn effect_validation_and_haptic_classification_follow_the_pinned_limits() {
        let effect = |duration: i32, amplitude: f32, frequency: f32, repeat: i32| {
            let mut p = Parcel::new();
            p.write_i32(MONO);
            one_shot(&mut p, duration, amplitude, frequency, repeat);
            read(&p).unwrap()
        };
        assert!(effect(1000, 1.0, 0.0, -1).haptic);
        assert!(!effect(1001, 1.0, 0.0, -1).haptic);
        assert!(!effect(100, 1.0, 0.0, 0).haptic);
        for (time, amplitude, frequency, repeat) in [
            (-1, 1.0, 0.0, -1),
            (0, 1.0, 0.0, -1),
            (100, f32::NAN, 0.0, -1),
            (100, 1.0, f32::INFINITY, -1),
            (100, -1.0, -0.0, -1),
            (100, 1.0, 0.0, 1),
        ] {
            assert!(!effect(time, amplitude, frequency, repeat).valid);
        }
        assert!(effect(100, -1.0, 0.0, -1).valid);
    }

    #[test]
    fn appops_bypass_allows_only_ignored_mode() {
        for (flags, accepted) in [
            (0, &[0][..]),
            (1, &[0, 1][..]),
            (2, &[0][..]),
            (3, &[0, 1][..]),
        ] {
            let attrs = VibrationAttributes {
                flags,
                ..Default::default()
            };
            for mode in 0..=4 {
                assert_eq!(attrs.allows(mode), accepted.contains(&mode));
            }
        }
    }
}
