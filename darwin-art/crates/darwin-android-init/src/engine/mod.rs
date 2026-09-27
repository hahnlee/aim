//! init's action engine (system/core/init/action_manager.cpp, action.cpp,
//! and the queueing in init.cpp `SecondStageMain`).
//!
//! The engine owns what init's main thread owns for actions: the action
//! list, the event queue, the currently executing actions, the property
//! trigger switch and the property waiter. Everything with side effects
//! outside init's own bookkeeping goes through [`CommandExecutor`].

mod command;
mod dry_run;

use std::collections::{BTreeMap, VecDeque};

pub use command::{Command, ExecSpec, InitAction};
pub use dry_run::{DryRunExecutor, ExecutedCommand};

use crate::PropertyLookup;
use crate::props::area::AreaMemory;
use crate::props::service::{INIT_CONTEXT, RESTORECON_PROPERTY, VENDOR_INIT_CONTEXT};
use crate::props::{PropertyService, SetEffect};
use crate::rc::{Action, CommandSpec, InitScripts, ParsedScripts, Service, expand_props};

/// `kColdBootDoneProp`.
pub const COLD_BOOT_DONE_PROP: &str = "ro.cold_boot_done";
/// The property `load_persist_props` waits for.
pub const PERSISTENT_PROPERTIES_READY_PROP: &str = "ro.persistent_properties.ready";

/// Properties as the engine needs them: reads for triggers and expansion,
/// and init's own writes (the `setprop` builtin).
pub trait InitProperties: PropertyLookup {
    /// `SetProperty` from init; returns the property changes to notify
    /// (normally the one set), or init's error text.
    fn init_set(&mut self, name: &str, value: &str) -> Result<Vec<(String, String)>, String>;
}

impl<M: AreaMemory> InitProperties for PropertyService<M> {
    fn init_set(&mut self, name: &str, value: &str) -> Result<Vec<(String, String)>, String> {
        let outcome = PropertyService::init_set(self, name, value);
        if !outcome.is_success() {
            return Err(outcome.error.unwrap_or_default());
        }
        Ok(outcome
            .effects
            .into_iter()
            .filter_map(|effect| match effect {
                SetEffect::Changed { name, value } => Some((name, value)),
                _ => None,
            })
            .collect())
    }
}

impl InitProperties for BTreeMap<String, String> {
    fn init_set(&mut self, name: &str, value: &str) -> Result<Vec<(String, String)>, String> {
        if name.starts_with("ro.") && self.contains_key(name) {
            return Err("Read-only property was already set".to_string());
        }
        self.insert(name.to_string(), value.to_string());
        Ok(vec![(name.to_string(), value.to_string())])
    }
}

/// What one command runs under.
#[derive(Clone, Copy, Debug)]
pub struct Invocation<'a> {
    pub command: &'a Command,
    /// `Action::BuildTriggersString` of the running action.
    pub action: &'a str,
    pub file: &'a str,
    pub line: usize,
    /// `u:r:init:s0`, or `u:r:vendor_init:s0` when a vendor script's
    /// command runs in the vendor_init subcontext.
    pub context: &'a str,
    /// The current service list (for `class_start`, `start`, ...).
    pub services: &'a [Service],
}

/// How a command left init's main loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandFlow {
    Done,
    /// `exec` / `exec_start` started a service that blocks the action
    /// queue until [`ActionManager::exec_finished`].
    ExecServiceRunning,
}

/// The side-effecting half of init's builtins. The engine handles
/// `trigger`, `setprop`, `wait_for_prop`, the property-trigger switch and
/// merging APEX scripts; every command (including those) is still passed to
/// the executor afterwards so it can observe or extend them.
pub trait CommandExecutor {
    fn execute(&mut self, invocation: &Invocation<'_>) -> Result<CommandFlow, String>;
}

/// An event in init's queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Trigger(String),
    /// A property change; the empty name is "all property actions".
    PropertyChange {
        name: String,
        value: String,
    },
    /// A queued builtin action, by action id.
    Builtin(usize),
}

#[derive(Clone, Debug)]
struct ActionEntry {
    id: usize,
    action: Action,
    oneshot: bool,
    init_action: Option<InitAction>,
}

/// What [`ActionManager::execute_one_command`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Ran one command.
    Ran(ExecutedCommand),
    /// Nothing queued.
    Idle,
    /// Blocked on `wait_for_prop` / `load_persist_props` / coldboot.
    WaitingForProperty { name: String, value: String },
    /// Blocked until the running `exec` service exits.
    WaitingForExec,
}

/// init's `ActionManager` plus the main-loop state around it.
pub struct ActionManager {
    actions: Vec<ActionEntry>,
    next_id: usize,
    event_queue: VecDeque<Event>,
    executing: VecDeque<usize>,
    current_command: usize,
    property_triggers_enabled: bool,
    waiting_for: Option<(String, String)>,
    exec_running: bool,
    services: Vec<Service>,
    pending_apex: Option<ParsedScripts>,
    vendor_api_level: u32,
    shutdown: Option<String>,
}

impl ActionManager {
    /// Loads the boot scripts' actions and services; the APEX scripts are
    /// merged when `perform_apex_config` runs, as in init.
    pub fn new(scripts: InitScripts, vendor_api_level: u32) -> Self {
        let mut manager = Self {
            actions: Vec::new(),
            next_id: 0,
            event_queue: VecDeque::new(),
            executing: VecDeque::new(),
            current_command: 0,
            property_triggers_enabled: false,
            waiting_for: None,
            exec_running: false,
            services: scripts.boot.services,
            pending_apex: Some(scripts.apex),
            vendor_api_level,
            shutdown: None,
        };
        for action in scripts.boot.actions {
            manager.add_action(action, false, None);
        }
        manager
    }

    fn add_action(
        &mut self,
        action: Action,
        oneshot: bool,
        init_action: Option<InitAction>,
    ) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.actions.push(ActionEntry {
            id,
            action,
            oneshot,
            init_action,
        });
        id
    }

    pub fn services(&self) -> &[Service] {
        &self.services
    }

    pub fn actions(&self) -> impl Iterator<Item = &Action> {
        self.actions.iter().map(|entry| &entry.action)
    }

    pub fn property_triggers_enabled(&self) -> bool {
        self.property_triggers_enabled
    }

    /// `QueueEventTrigger`.
    pub fn queue_event_trigger(&mut self, trigger: &str) {
        self.event_queue
            .push_back(Event::Trigger(trigger.to_string()));
    }

    /// `QueuePropertyChange`.
    pub fn queue_property_change(&mut self, name: &str, value: &str) {
        self.event_queue.push_back(Event::PropertyChange {
            name: name.to_string(),
            value: value.to_string(),
        });
    }

    /// `QueueBuiltinAction`: a oneshot action named after the builtin.
    pub fn queue_builtin_action(&mut self, init_action: InitAction) {
        let action = Action {
            event_trigger: init_action.name().to_string(),
            property_triggers: BTreeMap::new(),
            commands: vec![CommandSpec {
                builtin: crate::rc::Builtin::Trigger,
                args: vec![init_action.name().to_string()],
                line: 0,
            }],
            filename: "<Builtin Action>".to_string(),
            line: 0,
            vendor_subcontext: false,
        };
        let id = self.add_action(action, true, Some(init_action));
        self.event_queue.push_back(Event::Builtin(id));
    }

    /// The event queue `SecondStageMain` sets up before its main loop.
    pub fn queue_boot(&mut self, properties: &dyn PropertyLookup) {
        self.queue_builtin_action(InitAction::SetupCgroups);
        self.queue_builtin_action(InitAction::SetKptrRestrict);
        self.queue_builtin_action(InitAction::TestPerfEventSelinux);
        self.queue_event_trigger("early-init");
        self.queue_builtin_action(InitAction::ConnectEarlyStageSnapuserd);
        self.queue_builtin_action(InitAction::WaitForColdbootDone);
        self.queue_builtin_action(InitAction::CheckTradeInModeStatus);
        self.queue_builtin_action(InitAction::SetMmapRndBits);
        self.queue_builtin_action(InitAction::KeychordInit);
        self.queue_event_trigger("init");
        if properties.property_or("ro.bootmode", "") == "charger" {
            self.queue_event_trigger("charger");
        } else {
            self.queue_event_trigger("late-init");
        }
        self.queue_builtin_action(InitAction::QueuePropertyTriggers);
    }

    /// `PropertyChanged` (init.cpp): the property service calls this for
    /// every stored change. `sys.powerctl` requests shutdown immediately.
    pub fn property_changed(&mut self, name: &str, value: &str) {
        if name == "sys.powerctl" {
            self.shutdown = Some(value.to_string());
        }
        if self.property_triggers_enabled {
            self.queue_property_change(name, value);
        }
        if let Some((wait_name, wait_value)) = &self.waiting_for
            && wait_name == name
            && wait_value == value
        {
            self.waiting_for = None;
        }
    }

    /// A pending `sys.powerctl` command (`ShutdownState::CheckShutdown`).
    pub fn take_shutdown_request(&mut self) -> Option<String> {
        self.shutdown.take()
    }

    /// The `exec` / `exec_start` service exited.
    pub fn exec_finished(&mut self) {
        self.exec_running = false;
    }

    /// `PropWaiterState::StartWaiting`.
    fn start_waiting(&mut self, name: &str, value: &str, properties: &dyn PropertyLookup) -> bool {
        if self.waiting_for.is_some() {
            return false;
        }
        if properties.property_or(name, "") != value {
            self.waiting_for = Some((name.to_string(), value.to_string()));
        }
        true
    }

    /// `ActionManager::HasMoreCommands`.
    pub fn has_more_commands(&self) -> bool {
        !self.executing.is_empty() || !self.event_queue.is_empty()
    }

    /// `Action::CheckPropertyTriggers`.
    fn check_property_triggers(
        action: &Action,
        name: &str,
        value: &str,
        properties: &dyn PropertyLookup,
    ) -> bool {
        if action.property_triggers.is_empty() {
            return true;
        }
        if !name.is_empty() {
            match action.property_triggers.get(name) {
                None => return false,
                Some(trigger_value) => {
                    if trigger_value != "*" && trigger_value != value {
                        return false;
                    }
                }
            }
        }
        for (trigger_name, trigger_value) in &action.property_triggers {
            if trigger_name != name {
                let current = properties.property_or(trigger_name, "");
                if trigger_value == "*" && !current.is_empty() {
                    continue;
                }
                if *trigger_value != current {
                    return false;
                }
            }
        }
        true
    }

    fn check_event(entry: &ActionEntry, event: &Event, properties: &dyn PropertyLookup) -> bool {
        match event {
            Event::Trigger(trigger) => {
                *trigger == entry.action.event_trigger
                    && Self::check_property_triggers(&entry.action, "", "", properties)
            }
            Event::PropertyChange { name, value } => {
                entry.action.event_trigger.is_empty()
                    && Self::check_property_triggers(&entry.action, name, value, properties)
            }
            Event::Builtin(id) => entry.id == *id,
        }
    }

    /// `ActionManager::ExecuteOneCommand`, gated like init's main loop on
    /// the property waiter and a running exec service.
    pub fn execute_one_command(
        &mut self,
        properties: &mut dyn InitProperties,
        executor: &mut dyn CommandExecutor,
    ) -> Step {
        if let Some((name, value)) = &self.waiting_for {
            return Step::WaitingForProperty {
                name: name.clone(),
                value: value.clone(),
            };
        }
        if self.exec_running {
            return Step::WaitingForExec;
        }
        while self.executing.is_empty() {
            let Some(event) = self.event_queue.front().cloned() else {
                break;
            };
            for entry in &self.actions {
                if Self::check_event(entry, &event, &*properties) {
                    self.executing.push_back(entry.id);
                }
            }
            self.event_queue.pop_front();
        }
        let Some(&id) = self.executing.front() else {
            return Step::Idle;
        };
        let Some(index) = self.actions.iter().position(|entry| entry.id == id) else {
            // Removed while queued: skip it.
            self.executing.pop_front();
            self.current_command = 0;
            return self.execute_one_command(properties, executor);
        };
        let entry = self.actions[index].clone();
        let command_index = self.current_command;
        let executed = self.execute_command(&entry, command_index, properties, executor);

        self.current_command += 1;
        if self.current_command == entry.action.commands.len() {
            self.executing.pop_front();
            self.current_command = 0;
            if entry.oneshot {
                self.actions.retain(|candidate| candidate.id != id);
            }
        }
        Step::Ran(executed)
    }

    fn execute_command(
        &mut self,
        entry: &ActionEntry,
        index: usize,
        properties: &mut dyn InitProperties,
        executor: &mut dyn CommandExecutor,
    ) -> ExecutedCommand {
        let spec = &entry.action.commands[index];
        let triggers = entry.action.triggers_string();
        let context = if entry.action.vendor_subcontext {
            VENDOR_INIT_CONTEXT
        } else {
            INIT_CONTEXT
        };
        let mut record = ExecutedCommand {
            action: triggers.clone(),
            file: entry.action.filename.clone(),
            line: spec.line,
            command: spec.command_string(),
            expanded: Vec::new(),
            result: Ok(()),
        };

        let command = if let Some(init_action) = entry.init_action {
            Command::Init(init_action)
        } else {
            // RunBuiltinFunction: args[0] as is, the rest expanded.
            let mut expanded = vec![spec.args[0].clone()];
            for arg in &spec.args[1..] {
                match expand_props(arg, &*properties, self.vendor_api_level) {
                    Ok(value) => expanded.push(value),
                    Err(error) => {
                        record.result = Err(error);
                        return record;
                    }
                }
            }
            record.expanded = expanded.clone();
            match Command::from_args(spec.builtin, &expanded) {
                Ok(command) => command,
                Err(error) => {
                    record.result = Err(error);
                    return record;
                }
            }
        };

        if let Err(error) = self.run_internal(&command, properties) {
            record.result = Err(error);
            return record;
        }
        let services = std::mem::take(&mut self.services);
        let result = executor.execute(&Invocation {
            command: &command,
            action: &triggers,
            file: &entry.action.filename,
            line: spec.line,
            context,
            services: &services,
        });
        self.services = services;
        match result {
            Ok(CommandFlow::Done) => {}
            Ok(CommandFlow::ExecServiceRunning) => self.exec_running = true,
            Err(error) => record.result = Err(error),
        }
        if record.result.is_ok() {
            self.after_executor(&command, &*properties);
        }
        record
    }

    /// The parts of builtins that are init's own state.
    fn run_internal(
        &mut self,
        command: &Command,
        properties: &mut dyn InitProperties,
    ) -> Result<(), String> {
        match command {
            Command::Trigger { event } => self.queue_event_trigger(event),
            Command::Setprop { name, value } => {
                if name.starts_with("ctl.") {
                    return Err(
                        "Cannot set ctl. properties from init; call the Service functions directly"
                            .to_string(),
                    );
                }
                if name == RESTORECON_PROPERTY {
                    return Err(format!(
                        "Cannot set '{RESTORECON_PROPERTY}' from init; use the restorecon builtin directly"
                    ));
                }
                // SetProperty's failures are only logged by the service.
                if let Ok(changes) = properties.init_set(name, value) {
                    for (name, value) in changes {
                        self.property_changed(&name, &value);
                    }
                }
            }
            Command::WaitForProp { name, value } => {
                if !crate::props::is_legal_property_name(name) {
                    return Err(format!("IsLegalPropertyName({name}) failed"));
                }
                if value.len() >= crate::props::PROP_VALUE_MAX {
                    return Err("value too long".to_string());
                }
                if !self.start_waiting(name, value, &*properties) {
                    return Err("already waiting for a property".to_string());
                }
            }
            Command::Init(InitAction::WaitForColdbootDone) => {
                self.start_waiting(COLD_BOOT_DONE_PROP, "true", &*properties);
            }
            Command::Init(InitAction::QueuePropertyTriggers) => {
                self.queue_builtin_action(InitAction::EnablePropertyTrigger);
                self.queue_property_change("", "");
            }
            Command::Init(InitAction::EnablePropertyTrigger) => {
                self.property_triggers_enabled = true;
            }
            Command::PerformApexConfig { bootstrap: false } => {
                if let Some(apex) = self.pending_apex.take() {
                    self.services = apex.services;
                    for action in apex.actions {
                        self.add_action(action, false, None);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Waits that begin after the executor did its part.
    fn after_executor(&mut self, command: &Command, properties: &dyn PropertyLookup) {
        if let Command::LoadPersistProps = command {
            self.start_waiting(PERSISTENT_PROPERTIES_READY_PROP, "true", properties);
        }
    }

    /// Runs commands until init would sleep: nothing queued, or blocked on a
    /// property or an exec service. `limit` bounds runaway trigger loops.
    pub fn run_until_blocked(
        &mut self,
        properties: &mut dyn InitProperties,
        executor: &mut dyn CommandExecutor,
        limit: usize,
    ) -> (Vec<ExecutedCommand>, Step) {
        let mut ran = Vec::new();
        for _ in 0..limit {
            match self.execute_one_command(properties, executor) {
                Step::Ran(command) => ran.push(command),
                other => return (ran, other),
            }
        }
        (ran, Step::Idle)
    }
}

#[cfg(test)]
mod tests;
