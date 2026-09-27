//! init's property service rules (system/core/init/property_service.cpp):
//! `CheckPermissions`, `HandlePropertySet`, `PropertySet`.
//!
//! The service decides and records; it performs no I/O. Its caller (the
//! daemon's socket threads and init's main loop) acts on [`SetEffect`]s.

use super::area::{AreaMemory, HeapMemory};
use super::areas::{FutexWaker, NoWake, PropertyAreas};
use super::protocol::{
    PROP_ERROR_HANDLE_CONTROL_MESSAGE, PROP_ERROR_INVALID_NAME, PROP_ERROR_INVALID_VALUE,
    PROP_ERROR_PERMISSION_DENIED, PROP_ERROR_READ_ONLY_PROPERTY, PROP_ERROR_SET_FAILED,
    PROP_SUCCESS,
};
use super::{check_type, is_legal_property_name, is_legal_property_value};
use crate::PropertyLookup;

/// `kInitContext` / `kVendorContext`.
pub const INIT_CONTEXT: &str = "u:r:init:s0";
pub const VENDOR_INIT_CONTEXT: &str = "u:r:vendor_init:s0";
/// `kRestoreconProperty`.
pub const RESTORECON_PROPERTY: &str = "selinux.restorecon_recursive";

/// `struct ucred` of the peer (`SO_PEERCRED`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ucred {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
}

impl Ucred {
    /// init itself: `{.pid = 1, .uid = 0, .gid = 0}`.
    pub const INIT: Ucred = Ucred {
        pid: 1,
        uid: 0,
        gid: 0,
    };
}

/// The SELinux `property_service { set }` decision (`CheckMacPerms`).
///
/// ADR 0012 runs SELinux permissive, so [`Permissive`] allows everything;
/// a policy engine can replace it without touching the set rules.
pub trait PropertyPolicy {
    /// May `source_context` set a property labeled `target_context`?
    /// `name` is the property, or `ctl.<service>` / `ctl.<action>$<service>`
    /// for control messages. init denies when either context is missing.
    fn can_set(
        &self,
        source_context: &str,
        target_context: Option<&str>,
        name: &str,
        cred: &Ucred,
    ) -> bool;
}

/// Allows every set (SELinux permissive).
#[derive(Clone, Copy, Debug, Default)]
pub struct Permissive;

impl PropertyPolicy for Permissive {
    fn can_set(&self, _: &str, _: Option<&str>, _: &str, _: &Ucred) -> bool {
        true
    }
}

/// A `ctl.<action>` request for init's main loop (`QueueControlMessage`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlMessage {
    /// `start`, `stop`, `restart`, `interface_start`, `sigstop_on`,
    /// `apex_load`, ... (the property name after `ctl.`).
    pub action: String,
    /// The service, interface or APEX name (the property value).
    pub target: String,
    pub from_pid: i32,
}

/// Something the caller must do after a set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetEffect {
    /// `NotifyPropertyChange`: hand to the action manager
    /// (`PropertyChanged`), which queues property triggers once enabled and
    /// handles `sys.powerctl`.
    Changed { name: String, value: String },
    /// Queue for init's main loop; for `PROP_MSG_SETPROP2` the reply is sent
    /// when init has handled it (`PROP_SUCCESS` or
    /// `PROP_ERROR_HANDLE_CONTROL_MESSAGE`).
    Control(ControlMessage),
    /// Run `restorecon -R` on the path, then set the property to the path.
    Restorecon(String),
    /// Persist `persist.*` / `next_boot.*` to `/data/property`.
    Persist { name: String, value: String },
}

/// Result of one set request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetOutcome {
    /// `PROP_SUCCESS` or a `PROP_ERROR_*`; `None` when the reply is sent
    /// later (control messages and asynchronous persist writes).
    pub reply: Option<u32>,
    /// init's log text for a failure.
    pub error: Option<String>,
    pub effects: Vec<SetEffect>,
}

impl SetOutcome {
    fn code(code: u32) -> Self {
        Self {
            reply: Some(code),
            error: None,
            effects: Vec::new(),
        }
    }

    fn failure(code: u32, error: impl Into<String>) -> Self {
        Self {
            reply: Some(code),
            error: Some(error.into()),
            effects: Vec::new(),
        }
    }

    pub fn is_success(&self) -> bool {
        self.reply == Some(PROP_SUCCESS)
    }
}

/// The property service over a set of areas.
pub struct PropertyService<M: AreaMemory = HeapMemory> {
    areas: PropertyAreas<M>,
    waker: Box<dyn FutexWaker>,
    policy: Box<dyn PropertyPolicy>,
    /// `persistent_properties_loaded`: set once `load_persist_props` has
    /// loaded `/data/property/persistent_properties`.
    pub persistent_properties_loaded: bool,
    /// `ro.property_service.async_persist_writes`: persist writes reply later.
    pub async_persist_writes: bool,
    /// `accept_messages`: cleared at shutdown.
    pub accept_messages: bool,
    /// `SelinuxGetVendorAndroidVersion() > Q`: control-message replies are
    /// deferred to init's main loop.
    pub deferred_control_replies: bool,
}

impl PropertyService<HeapMemory> {
    pub fn new(property_info: Vec<u8>) -> Result<Self, String> {
        Ok(Self::with_areas(PropertyAreas::new(property_info)?))
    }
}

impl<M: AreaMemory> PropertyService<M> {
    pub fn with_areas(areas: PropertyAreas<M>) -> Self {
        Self {
            areas,
            waker: Box::new(NoWake),
            policy: Box::new(Permissive),
            persistent_properties_loaded: false,
            async_persist_writes: false,
            accept_messages: true,
            deferred_control_replies: true,
        }
    }

    pub fn set_waker(&mut self, waker: Box<dyn FutexWaker>) {
        self.waker = waker;
    }

    pub fn set_policy(&mut self, policy: Box<dyn PropertyPolicy>) {
        self.policy = policy;
    }

    pub fn areas(&self) -> &PropertyAreas<M> {
        &self.areas
    }

    fn mac_allows(&self, name: &str, target: Option<&str>, source: &str, cred: &Ucred) -> bool {
        // CheckMacPerms: no target or source context means deny.
        if target.is_none() || source.is_empty() {
            return false;
        }
        self.policy.can_set(source, target, name, cred)
    }

    /// `CheckControlPropertyPerms`.
    fn check_control_perms(&self, name: &str, value: &str, source: &str, cred: &Ucred) -> bool {
        let info = self.areas.info_area();
        if matches!(name, "ctl.start" | "ctl.stop" | "ctl.restart") {
            let legacy = format!("ctl.{value}");
            let (target, _) = info.property_info(&legacy);
            if self.mac_allows(&legacy, target, source, cred) {
                return true;
            }
        }
        let full = format!("{name}${value}");
        let (target, _) = info.property_info(&full);
        self.mac_allows(&full, target, source, cred)
    }

    /// `CheckPermissions`: returns `PROP_SUCCESS` or an error code and text.
    pub fn check_permissions(
        &self,
        name: &str,
        value: &str,
        source_context: &str,
        cred: &Ucred,
    ) -> Result<(), (u32, String)> {
        if !is_legal_property_name(name) {
            return Err((PROP_ERROR_INVALID_NAME, "Illegal property name".to_string()));
        }
        if let Some(action) = name.strip_prefix("ctl.") {
            if !self.check_control_perms(name, value, source_context, cred) {
                return Err((
                    PROP_ERROR_HANDLE_CONTROL_MESSAGE,
                    format!("Invalid permissions to perform '{action}' on '{value}'"),
                ));
            }
            return Ok(());
        }
        let (target, type_) = self.areas.info_area().property_info(name);
        if !self.mac_allows(name, target, source_context, cred) {
            return Err((
                PROP_ERROR_PERMISSION_DENIED,
                "SELinux permission check failed".to_string(),
            ));
        }
        let type_ = type_.unwrap_or("(null)");
        if !check_type(type_, value) {
            return Err((
                PROP_ERROR_INVALID_VALUE,
                format!("Property type check failed, value doesn't match expected type '{type_}'"),
            ));
        }
        Ok(())
    }

    /// `HandlePropertySet` for a request from a socket peer (or from init
    /// with `from_socket == false`).
    pub fn handle_set(
        &mut self,
        name: &str,
        value: &[u8],
        source_context: &str,
        cred: &Ucred,
        from_socket: bool,
    ) -> SetOutcome {
        let value_str = String::from_utf8_lossy(value).into_owned();
        if let Err((code, error)) = self.check_permissions(name, &value_str, source_context, cred) {
            return SetOutcome::failure(code, error);
        }
        if let Some(action) = name.strip_prefix("ctl.") {
            return self.send_control_message(action, &value_str, cred, from_socket);
        }
        if name == "sys.powerctl" && value_str == "reboot,userspace" {
            return SetOutcome::failure(
                PROP_ERROR_INVALID_VALUE,
                "Userspace reboot is deprecated.",
            );
        }
        if name == RESTORECON_PROPERTY && cred.pid != 1 && !value.is_empty() {
            let mut outcome = SetOutcome::code(PROP_SUCCESS);
            outcome.effects.push(SetEffect::Restorecon(value_str));
            return outcome;
        }
        self.property_set(name, value, from_socket)
    }

    /// `SendControlMessage`.
    fn send_control_message(
        &self,
        action: &str,
        target: &str,
        cred: &Ucred,
        from_socket: bool,
    ) -> SetOutcome {
        if !self.accept_messages {
            if action == "stop" {
                return SetOutcome::code(PROP_SUCCESS);
            }
            return SetOutcome::failure(
                PROP_ERROR_HANDLE_CONTROL_MESSAGE,
                "Received control message after shutdown, ignoring",
            );
        }
        let deferred = from_socket && self.deferred_control_replies;
        SetOutcome {
            reply: if deferred { None } else { Some(PROP_SUCCESS) },
            error: None,
            effects: vec![SetEffect::Control(ControlMessage {
                action: action.to_string(),
                target: target.to_string(),
                from_pid: cred.pid,
            })],
        }
    }

    /// `PropertySet`: the write itself, without permission checks.
    pub fn property_set(&mut self, name: &str, value: &[u8], from_socket: bool) -> SetOutcome {
        if !is_legal_property_name(name) {
            return SetOutcome::failure(PROP_ERROR_INVALID_NAME, "Illegal property name");
        }
        if let Err(error) = is_legal_property_value(name, value) {
            return SetOutcome::failure(PROP_ERROR_INVALID_VALUE, error);
        }
        let value_str = String::from_utf8_lossy(value).into_owned();
        let mut outcome = SetOutcome::code(PROP_SUCCESS);
        // sys.powerctl is never stored: the change notification handles it.
        if name != "sys.powerctl" {
            if self.areas.contains(name) {
                // ro.* properties are write-once.
                if name.starts_with("ro.") {
                    return SetOutcome::failure(
                        PROP_ERROR_READ_ONLY_PROPERTY,
                        "Read-only property was already set",
                    );
                }
                if let Err(error) = self.areas.update(name, value, self.waker.as_ref()) {
                    // __system_property_update's result is ignored by init.
                    outcome.error = Some(format!("__system_property_update failed: {error:?}"));
                }
            } else if let Err(error) = self.areas.add(name, value, self.waker.as_ref()) {
                return SetOutcome::failure(
                    PROP_ERROR_SET_FAILED,
                    format!("__system_property_add failed: {error:?}"),
                );
            }
            let need_persist = name.starts_with("persist.") || name.starts_with("next_boot.");
            if from_socket && self.persistent_properties_loaded && need_persist {
                outcome.effects.push(SetEffect::Persist {
                    name: name.to_string(),
                    value: value_str.clone(),
                });
                if self.async_persist_writes {
                    // PersistWriteThread notifies and replies after writing.
                    outcome.reply = None;
                    return outcome;
                }
            }
        }
        if self.accept_messages {
            outcome.effects.push(SetEffect::Changed {
                name: name.to_string(),
                value: value_str,
            });
        }
        outcome
    }

    /// `InitPropertySet`: init setting a property as itself.
    pub fn init_set(&mut self, name: &str, value: &str) -> SetOutcome {
        self.handle_set(name, value.as_bytes(), INIT_CONTEXT, &Ucred::INIT, false)
    }

    /// `PropertySetNoSocket`: used while loading `.prop` files and deriving
    /// defaults (no permission checks).
    pub fn set_no_socket(&mut self, name: &str, value: &str) -> SetOutcome {
        self.property_set(name, value.as_bytes(), false)
    }

    /// `__system_property_find` / `GetProperty`.
    pub fn get(&self, name: &str) -> Option<String> {
        self.areas.get(name)
    }
}

impl<M: AreaMemory> PropertyLookup for PropertyService<M> {
    fn property(&self, name: &str) -> Option<String> {
        self.areas.get(name)
    }
}
