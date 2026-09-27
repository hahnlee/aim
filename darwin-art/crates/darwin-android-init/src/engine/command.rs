//! Typed builtin commands, built from a [`CommandSpec`] after init's
//! property expansion (`RunBuiltinFunction`).
//!
//! Each variant carries the arguments as the builtin receives them; where
//! the builtin itself validates or decodes (modes, owners, fstab options,
//! mkdir encryption options), the strings are kept for the executor, which
//! owns that behavior.

use crate::rc::{Builtin, Rlimit, parse_rlimit};

/// `exec [ <seclabel> [ <user> [ <group>* ] ] -- ] <command> [ <arg>* ]`
/// (`Service::MakeTemporaryOneshotService`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecSpec {
    /// `None` for the default (no seclabel, or `-`).
    pub seclabel: Option<String>,
    pub user: Option<String>,
    pub group: Option<String>,
    pub supplementary_groups: Vec<String>,
    /// Program and arguments.
    pub args: Vec<String>,
}

impl ExecSpec {
    fn parse(args: &[String]) -> Result<Self, String> {
        let command_arg = args
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, arg)| *arg == "--")
            .map_or(1, |(index, _)| index + 1);
        if command_arg > 4 + 32 {
            return Err("exec called with too many supplementary group ids".to_string());
        }
        if command_arg >= args.len() {
            return Err("exec called without command".to_string());
        }
        let seclabel = (command_arg > 2 && args[1] != "-").then(|| args[1].clone());
        let user = (command_arg > 3).then(|| args[2].clone());
        let (group, supplementary_groups) = if command_arg > 4 {
            (Some(args[3].clone()), args[4..command_arg - 1].to_vec())
        } else {
            (None, Vec::new())
        };
        Ok(Self {
            seclabel,
            user,
            group,
            supplementary_groups,
            args: args[command_arg..].to_vec(),
        })
    }
}

/// init's own queued builtin actions (`QueueBuiltinAction` in
/// `SecondStageMain`), which have no `.rc` source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InitAction {
    SetupCgroups,
    SetKptrRestrict,
    TestPerfEventSelinux,
    ConnectEarlyStageSnapuserd,
    /// Waits for `ro.cold_boot_done=true` (set when ueventd finishes).
    WaitForColdbootDone,
    CheckTradeInModeStatus,
    SetMmapRndBits,
    KeychordInit,
    /// Queues `enable_property_trigger` and an all-properties event.
    QueuePropertyTriggers,
    EnablePropertyTrigger,
}

impl InitAction {
    /// The name init queues it under (also its event trigger).
    pub fn name(self) -> &'static str {
        match self {
            InitAction::SetupCgroups => "SetupCgroups",
            InitAction::SetKptrRestrict => "SetKptrRestrict",
            InitAction::TestPerfEventSelinux => "TestPerfEventSelinux",
            InitAction::ConnectEarlyStageSnapuserd => "ConnectEarlyStageSnapuserd",
            InitAction::WaitForColdbootDone => "wait_for_coldboot_done",
            InitAction::CheckTradeInModeStatus => "CheckTradeInModeStatus",
            InitAction::SetMmapRndBits => "SetMmapRndBits",
            InitAction::KeychordInit => "KeychordInit",
            InitAction::QueuePropertyTriggers => "queue_property_triggers",
            InitAction::EnablePropertyTrigger => "enable_property_trigger",
        }
    }
}

/// One command, ready to execute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Bootchart {
        action: String,
    },
    Chmod {
        mode: String,
        path: String,
    },
    Chown {
        owner: String,
        group: Option<String>,
        path: String,
    },
    ClassReset {
        class: String,
    },
    ClassRestart {
        class: String,
        only_enabled: bool,
    },
    ClassStart {
        class: String,
    },
    ClassStop {
        class: String,
    },
    Copy {
        source: String,
        target: String,
    },
    CopyPerLine {
        source: String,
        target: String,
    },
    Domainname {
        name: String,
    },
    Enable {
        service: String,
    },
    Exec(ExecSpec),
    ExecBackground(ExecSpec),
    ExecStart {
        service: String,
    },
    Export {
        name: String,
        value: String,
    },
    Hostname {
        name: String,
    },
    Ifup {
        interface: String,
    },
    InitUser0,
    Insmod {
        force: bool,
        path: String,
        options: String,
    },
    Installkey {
        dir: String,
    },
    InterfaceRestart {
        interface: String,
    },
    InterfaceStart {
        interface: String,
    },
    InterfaceStop {
        interface: String,
    },
    LoadExports {
        path: String,
    },
    LoadPersistProps,
    LoadSystemProps,
    Loglevel {
        level: String,
    },
    MarkPostData,
    /// `mkdir <path> [<mode> [<owner> [<group> [encryption=... key=...]]]]`
    /// (`ParseMkdir`).
    Mkdir {
        path: String,
        mode: Option<String>,
        owner: Option<String>,
        group: Option<String>,
        options: Vec<String>,
    },
    MountAll {
        args: Vec<String>,
    },
    Mount {
        fs_type: String,
        device: String,
        target: String,
        options: Vec<String>,
    },
    PerformApexConfig {
        bootstrap: bool,
    },
    Umount {
        path: String,
    },
    UmountAll {
        fstab: Option<String>,
    },
    UpdateLinkerConfig,
    Readahead {
        path: String,
        fully: bool,
    },
    Restart {
        service: String,
        only_if_running: bool,
    },
    Restorecon {
        args: Vec<String>,
    },
    RestoreconRecursive {
        args: Vec<String>,
    },
    Rm {
        path: String,
    },
    Rmdir {
        path: String,
    },
    Setprop {
        name: String,
        value: String,
    },
    Setrlimit(Rlimit),
    Start {
        service: String,
    },
    Stop {
        service: String,
    },
    SwaponAll {
        fstab: Option<String>,
    },
    Swapoff {
        path: String,
    },
    EnterDefaultMountNs,
    Symlink {
        target: String,
        link: String,
    },
    Sysclktz {
        minutes_west: String,
    },
    Trigger {
        event: String,
    },
    VerityUpdateState,
    Wait {
        path: String,
        timeout: Option<String>,
    },
    WaitForProp {
        name: String,
        value: String,
    },
    Write {
        path: String,
        content: String,
    },
    /// init's internal builtin actions.
    Init(InitAction),
}

impl Command {
    /// Builds the typed command from expanded arguments (`args[0]` is the
    /// keyword; arity was checked at parse time). Errors are the builtin's
    /// own argument errors.
    pub fn from_args(builtin: Builtin, args: &[String]) -> Result<Self, String> {
        let a = |i: usize| args[i].clone();
        let opt = |i: usize| args.get(i).cloned();
        Ok(match builtin {
            Builtin::Bootchart => Command::Bootchart { action: a(1) },
            Builtin::Chmod => Command::Chmod {
                mode: a(1),
                path: a(2),
            },
            Builtin::Chown => {
                if args.len() == 3 {
                    Command::Chown {
                        owner: a(1),
                        group: None,
                        path: a(2),
                    }
                } else {
                    Command::Chown {
                        owner: a(1),
                        group: Some(a(2)),
                        path: a(3),
                    }
                }
            }
            Builtin::ClassReset => Command::ClassReset { class: a(1) },
            Builtin::ClassRestart => {
                if args.len() == 3 {
                    if args[1] != "--only-enabled" {
                        return Err(format!("Unexpected argument: {}", args[1]));
                    }
                    Command::ClassRestart {
                        class: a(2),
                        only_enabled: true,
                    }
                } else {
                    Command::ClassRestart {
                        class: a(1),
                        only_enabled: false,
                    }
                }
            }
            Builtin::ClassStart => Command::ClassStart { class: a(1) },
            Builtin::ClassStop => Command::ClassStop { class: a(1) },
            Builtin::Copy => Command::Copy {
                source: a(1),
                target: a(2),
            },
            Builtin::CopyPerLine => Command::CopyPerLine {
                source: a(1),
                target: a(2),
            },
            Builtin::Domainname => Command::Domainname { name: a(1) },
            Builtin::Enable => Command::Enable { service: a(1) },
            Builtin::Exec => Command::Exec(ExecSpec::parse(args)?),
            Builtin::ExecBackground => Command::ExecBackground(ExecSpec::parse(args)?),
            Builtin::ExecStart => Command::ExecStart { service: a(1) },
            Builtin::Export => Command::Export {
                name: a(1),
                value: a(2),
            },
            Builtin::Hostname => Command::Hostname { name: a(1) },
            Builtin::Ifup => Command::Ifup { interface: a(1) },
            Builtin::InitUser0 => Command::InitUser0,
            Builtin::Insmod => {
                let force = args[1] == "-f";
                let first = if force { 2 } else { 1 };
                let path = args
                    .get(first)
                    .cloned()
                    .ok_or_else(|| "insmod: missing module path".to_string())?;
                Command::Insmod {
                    force,
                    path,
                    options: args[first + 1..].join(" "),
                }
            }
            Builtin::Installkey => Command::Installkey { dir: a(1) },
            Builtin::InterfaceRestart => Command::InterfaceRestart { interface: a(1) },
            Builtin::InterfaceStart => Command::InterfaceStart { interface: a(1) },
            Builtin::InterfaceStop => Command::InterfaceStop { interface: a(1) },
            Builtin::LoadExports => Command::LoadExports { path: a(1) },
            Builtin::LoadPersistProps => Command::LoadPersistProps,
            Builtin::LoadSystemProps => Command::LoadSystemProps,
            Builtin::Loglevel => Command::Loglevel { level: a(1) },
            Builtin::MarkPostData => Command::MarkPostData,
            Builtin::Mkdir => Command::Mkdir {
                path: a(1),
                mode: opt(2),
                owner: opt(3),
                group: opt(4),
                options: args.get(5..).map(<[String]>::to_vec).unwrap_or_default(),
            },
            Builtin::MountAll => Command::MountAll {
                args: args[1..].to_vec(),
            },
            Builtin::Mount => Command::Mount {
                fs_type: a(1),
                device: a(2),
                target: a(3),
                options: args[4..].to_vec(),
            },
            Builtin::PerformApexConfig => {
                let bootstrap = match args.get(1).map(String::as_str) {
                    None => false,
                    Some("--bootstrap") => true,
                    Some(other) => return Err(format!("Unexpected argument: {other}")),
                };
                Command::PerformApexConfig { bootstrap }
            }
            Builtin::Umount => Command::Umount { path: a(1) },
            Builtin::UmountAll => Command::UmountAll { fstab: opt(1) },
            Builtin::UpdateLinkerConfig => Command::UpdateLinkerConfig,
            Builtin::Readahead => Command::Readahead {
                path: a(1),
                fully: args.len() == 3 && args[2] == "--fully",
            },
            Builtin::Restart => {
                if args.len() == 3 {
                    if args[1] != "--only-if-running" {
                        return Err(format!("Unknown argument to restart: {}", args[1]));
                    }
                    Command::Restart {
                        service: a(2),
                        only_if_running: true,
                    }
                } else {
                    Command::Restart {
                        service: a(1),
                        only_if_running: false,
                    }
                }
            }
            Builtin::Restorecon => Command::Restorecon {
                args: args[1..].to_vec(),
            },
            Builtin::RestoreconRecursive => Command::RestoreconRecursive {
                args: args[1..].to_vec(),
            },
            Builtin::Rm => Command::Rm { path: a(1) },
            Builtin::Rmdir => Command::Rmdir { path: a(1) },
            Builtin::Setprop => Command::Setprop {
                name: a(1),
                value: a(2),
            },
            Builtin::Setrlimit => Command::Setrlimit(parse_rlimit(args)?),
            Builtin::Start => Command::Start { service: a(1) },
            Builtin::Stop => Command::Stop { service: a(1) },
            Builtin::SwaponAll => Command::SwaponAll { fstab: opt(1) },
            Builtin::Swapoff => Command::Swapoff { path: a(1) },
            Builtin::EnterDefaultMountNs => Command::EnterDefaultMountNs,
            Builtin::Symlink => Command::Symlink {
                target: a(1),
                link: a(2),
            },
            Builtin::Sysclktz => Command::Sysclktz { minutes_west: a(1) },
            Builtin::Trigger => Command::Trigger { event: a(1) },
            Builtin::VerityUpdateState => Command::VerityUpdateState,
            Builtin::Wait => Command::Wait {
                path: a(1),
                timeout: opt(2),
            },
            Builtin::WaitForProp => Command::WaitForProp {
                name: a(1),
                value: a(2),
            },
            Builtin::Write => Command::Write {
                path: a(1),
                content: a(2),
            },
        })
    }
}
