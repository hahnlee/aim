//! `service` sections (system/core/init/service_parser.cpp).

use std::collections::BTreeSet;

use super::ParseEnv;
use super::builtins::CommandSpec;
use super::caps;
use super::expand::{ANDROID_API_R, expand_props};
use super::keywords::{self, UNBOUNDED};
use crate::libbase::{parse_int, parse_uint};

/// `NR_SVC_SUPP_GIDS`.
const NR_SVC_SUPP_GIDS: usize = 32;
/// `PROP_VALUE_MAX`.
const PROP_VALUE_MAX: usize = 92;
/// lmkd_service.h.
pub const MIN_OOM_SCORE_ADJUST: i64 = -1000;
pub const MAX_OOM_SCORE_ADJUST: i64 = 1000;
pub const DEFAULT_OOM_SCORE_ADJUST: i32 = -1000;
/// system/thread_defs.h.
const ANDROID_PRIORITY_HIGHEST: i64 = -20;
const ANDROID_PRIORITY_LOWEST: i64 = 19;
/// linux/input.h `KEY_MAX`.
const KEY_MAX: i64 = 0x2ff;

/// A user or group as written in the script, with the id init resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Id {
    pub name: String,
    pub id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketType {
    Stream,
    Dgram,
    Seqpacket,
}

/// `socket <name> <type> <perm> [ <user> [ <group> [ <seclabel> ] ] ]`:
/// init creates `/dev/socket/<name>` and passes the fd in
/// `ANDROID_SOCKET_<name>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SocketDescriptor {
    pub name: String,
    pub socket_type: SocketType,
    pub passcred: bool,
    pub listen: bool,
    /// Octal permission bits.
    pub perm: u32,
    /// Defaults to 0 (root) when not given.
    pub uid: Option<Id>,
    pub gid: Option<Id>,
    /// Empty when not given: init computes it from the file context.
    pub context: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileMode {
    Read,
    Write,
    ReadWrite,
}

/// `file <path> <r|w|rw>`: opened by init and passed in `ANDROID_FILE_<path>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDescriptor {
    /// Property-expanded absolute path.
    pub name: String,
    pub mode: FileMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IoSchedClass {
    #[default]
    None,
    Rt,
    Be,
    Idle,
}

/// `rlimit <resource> <cur> <max>` (rlimit_parser.cpp). Limits are
/// `u64::MAX` for `RLIM_INFINITY` (`-1` / `unlimited`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rlimit {
    /// Linux `RLIMIT_*` number.
    pub resource: u32,
    pub soft: u64,
    pub hard: u64,
}

/// Linux namespace flags a service asks for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Namespaces {
    pub new_pid: bool,
    pub new_mount: bool,
    /// `enter_namespace net <path>` (implies a new mount namespace).
    pub enter_net: Option<String>,
}

/// `critical [window=<minutes>] [target=<reboot target>]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Critical {
    /// Crashes allowed within this many minutes (default 4).
    pub window_minutes: i64,
    pub reboot_target: Option<String>,
}

/// An option kept exactly as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawOption {
    pub args: Vec<String>,
    pub line: usize,
}

/// One `service` definition after init's option parsing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    /// Program path followed by its arguments, unexpanded (init expands
    /// `${...}` in them when the service starts).
    pub args: Vec<String>,
    /// Guest path of the defining `.rc` file.
    pub filename: String,
    pub line: usize,
    /// Defaults to `{"default"}`.
    pub classnames: BTreeSet<String>,
    pub disabled: bool,
    pub oneshot: bool,
    pub critical: Option<Critical>,
    pub user: Option<Id>,
    pub group: Option<Id>,
    pub supplementary_groups: Vec<Id>,
    /// `None` without a `capabilities` line; `Some(0)` for an empty one.
    /// Bit n is Linux capability n.
    pub capabilities: Option<u64>,
    pub sockets: Vec<SocketDescriptor>,
    pub files: Vec<FileDescriptor>,
    pub onrestart: Vec<CommandSpec>,
    pub seclabel: Option<String>,
    pub writepid_files: Vec<String>,
    pub task_profiles: Vec<String>,
    /// `interface <name> <instance>` as `name/instance`.
    pub interfaces: BTreeSet<String>,
    pub ioprio_class: IoSchedClass,
    pub ioprio_priority: i32,
    pub priority: i32,
    pub rlimits: Vec<Rlimit>,
    pub environment: Vec<(String, String)>,
    pub namespaces: Namespaces,
    pub updatable: bool,
    pub is_override: bool,
    /// Seconds; init's default is 5.
    pub restart_period: u64,
    pub timeout_period: Option<u64>,
    pub oom_score_adjust: i32,
    /// `console [<tty>]`: `Some("")` for the default console.
    pub console: Option<String>,
    pub stdio_to_kmsg: bool,
    pub gentle_kill: bool,
    pub sigstop: bool,
    pub shutdown_critical: bool,
    pub reboot_on_failure: Option<String>,
    /// Upstream options this model does not type (`keycodes`, `memcg.*`,
    /// `shared_kallsyms`) and unknown options, verbatim.
    pub other_options: Vec<RawOption>,
    /// The file is under a vendor subcontext path (`/vendor`, `/odm`, or a
    /// vendor APEX): `onrestart` commands run in vendor_init.
    pub vendor_subcontext: bool,
}

impl Service {
    fn new(name: String, args: Vec<String>, filename: &str, line: usize, vendor: bool) -> Self {
        Self {
            name,
            args,
            filename: filename.to_string(),
            line,
            classnames: BTreeSet::from(["default".to_string()]),
            disabled: false,
            oneshot: false,
            critical: None,
            user: None,
            group: None,
            supplementary_groups: Vec::new(),
            capabilities: None,
            sockets: Vec::new(),
            files: Vec::new(),
            onrestart: Vec::new(),
            seclabel: None,
            writepid_files: Vec::new(),
            task_profiles: Vec::new(),
            interfaces: BTreeSet::new(),
            ioprio_class: IoSchedClass::None,
            ioprio_priority: 0,
            priority: 0,
            rlimits: Vec::new(),
            environment: Vec::new(),
            namespaces: Namespaces::default(),
            updatable: false,
            is_override: false,
            restart_period: 5,
            timeout_period: None,
            oom_score_adjust: DEFAULT_OOM_SCORE_ADJUST,
            console: None,
            stdio_to_kmsg: false,
            gentle_kill: false,
            sigstop: false,
            shutdown_critical: false,
            reboot_on_failure: None,
            other_options: Vec::new(),
            vendor_subcontext: vendor,
        }
    }

    /// The uid the service runs as (`ProcessAttributes::uid()`): root when
    /// no `user` was given.
    pub fn uid(&self) -> u32 {
        self.user.as_ref().map_or(0, |id| id.id)
    }

    pub fn gid(&self) -> u32 {
        self.group.as_ref().map_or(0, |id| id.id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Opt {
    Capabilities,
    Class,
    Console,
    Critical,
    Disabled,
    EnterNamespace,
    File,
    GentleKill,
    Group,
    Interface,
    Ioprio,
    Keycodes,
    Memcg,
    Namespace,
    Oneshot,
    Onrestart,
    OomScoreAdjust,
    Override,
    Priority,
    RebootOnFailure,
    RestartPeriod,
    Rlimit,
    Seclabel,
    Setenv,
    SharedKallsyms,
    Shutdown,
    Sigstop,
    Socket,
    StdioToKmsg,
    TaskProfiles,
    TimeoutPeriod,
    Updatable,
    User,
    Writepid,
}

/// `ServiceParser::GetParserMap`, verbatim ranges.
const OPTIONS: &[(&str, usize, usize, Opt)] = &[
    ("capabilities", 0, UNBOUNDED, Opt::Capabilities),
    ("class", 1, UNBOUNDED, Opt::Class),
    ("console", 0, 1, Opt::Console),
    ("critical", 0, 2, Opt::Critical),
    ("disabled", 0, 0, Opt::Disabled),
    ("enter_namespace", 2, 2, Opt::EnterNamespace),
    ("file", 2, 2, Opt::File),
    ("gentle_kill", 0, 0, Opt::GentleKill),
    ("group", 1, NR_SVC_SUPP_GIDS + 1, Opt::Group),
    ("interface", 2, 2, Opt::Interface),
    ("ioprio", 2, 2, Opt::Ioprio),
    ("keycodes", 1, UNBOUNDED, Opt::Keycodes),
    ("memcg.limit_in_bytes", 1, 1, Opt::Memcg),
    ("memcg.limit_percent", 1, 1, Opt::Memcg),
    ("memcg.limit_property", 1, 1, Opt::Memcg),
    ("memcg.soft_limit_in_bytes", 1, 1, Opt::Memcg),
    ("memcg.swappiness", 1, 1, Opt::Memcg),
    ("namespace", 1, 2, Opt::Namespace),
    ("oneshot", 0, 0, Opt::Oneshot),
    ("onrestart", 1, UNBOUNDED, Opt::Onrestart),
    ("oom_score_adjust", 1, 1, Opt::OomScoreAdjust),
    ("override", 0, 0, Opt::Override),
    ("priority", 1, 1, Opt::Priority),
    ("reboot_on_failure", 1, 1, Opt::RebootOnFailure),
    ("restart_period", 1, 1, Opt::RestartPeriod),
    ("rlimit", 3, 3, Opt::Rlimit),
    ("seclabel", 1, 1, Opt::Seclabel),
    ("setenv", 2, 2, Opt::Setenv),
    ("shared_kallsyms", 0, 0, Opt::SharedKallsyms),
    ("shutdown", 1, 1, Opt::Shutdown),
    ("sigstop", 0, 0, Opt::Sigstop),
    ("socket", 3, 6, Opt::Socket),
    ("stdio_to_kmsg", 0, 0, Opt::StdioToKmsg),
    ("task_profiles", 1, UNBOUNDED, Opt::TaskProfiles),
    ("timeout_period", 1, 1, Opt::TimeoutPeriod),
    ("updatable", 0, 0, Opt::Updatable),
    ("user", 1, 1, Opt::User),
    ("writepid", 1, UNBOUNDED, Opt::Writepid),
];

/// Outcome of one option line: init either accepts it, or logs an error and
/// keeps the service (the option's partial effect stays, as in init).
pub(crate) enum OptionOutcome {
    Ok,
    /// Accepted but noteworthy (init's `LOG(WARNING)`, or an option kept
    /// verbatim).
    Warning(String),
    Error(String),
}

/// `ServiceParser::IsValidName`.
pub(crate) fn is_valid_service_name(name: &str) -> bool {
    crate::props::is_legal_property_name(&format!("init.svc.{name}"))
        && name.len() <= PROP_VALUE_MAX
}

/// `ServiceParser::ParseSection`.
pub(crate) fn begin_service(
    args: &[String],
    filename: &str,
    line: usize,
    vendor_subcontext: bool,
    env: &ParseEnv<'_>,
) -> Result<Service, String> {
    if args.len() < 3 {
        return Err("services must have a name and a program".to_string());
    }
    let name = &args[1];
    if !is_valid_service_name(name) {
        return Err(format!("invalid service name '{name}'"));
    }
    let mut program: Vec<String> = args[2..].to_vec();
    if env.vendor_api_level <= super::expand::ANDROID_API_P && program[0] == "/sbin/watchdogd" {
        program[0] = "/system/bin/watchdogd".to_string();
    }
    if env.vendor_api_level <= super::expand::ANDROID_API_Q && program[0] == "/charger" {
        program[0] = "/system/bin/charger".to_string();
    }
    Ok(Service::new(
        name.clone(),
        program,
        filename,
        line,
        vendor_subcontext,
    ))
}

/// `ServiceParser::ParseLineSection`.
pub(crate) fn parse_option(
    service: &mut Service,
    args: Vec<String>,
    line: usize,
    existing: &[Service],
    env: &ParseEnv<'_>,
) -> OptionOutcome {
    let opt = match keywords::find(OPTIONS, &args) {
        Ok(opt) => *opt,
        Err(error) => {
            if error.starts_with("Invalid keyword") {
                // Not an Android 16 option: init rejects the line. Keep it.
                service.other_options.push(RawOption { args, line });
            }
            return OptionOutcome::Error(error);
        }
    };
    match apply_option(service, opt, args, line, existing, env) {
        Ok(None) => OptionOutcome::Ok,
        Ok(Some(warning)) => OptionOutcome::Warning(warning),
        Err(error) => OptionOutcome::Error(error),
    }
}

fn decode(env: &ParseEnv<'_>, name: &str) -> Result<Id, String> {
    env.ids.decode_uid(name).map(|id| Id {
        name: name.to_string(),
        id,
    })
}

fn apply_option(
    service: &mut Service,
    opt: Opt,
    mut args: Vec<String>,
    line: usize,
    existing: &[Service],
    env: &ParseEnv<'_>,
) -> Result<Option<String>, String> {
    match opt {
        Opt::Capabilities => {
            let mut set = 0u64;
            service.capabilities = Some(0);
            for arg in &args[1..] {
                let Some(cap) = caps::lookup_cap(arg) else {
                    return Err(format!("invalid capability '{arg}'"));
                };
                set |= 1 << cap;
                service.capabilities = Some(set);
            }
        }
        Opt::Class => {
            service.classnames = args[1..].iter().cloned().collect();
        }
        Opt::Console => {
            if service.stdio_to_kmsg {
                return Err("'console' and 'stdio_to_kmsg' are mutually exclusive".to_string());
            }
            service.console = Some(
                args.get(1)
                    .map_or(String::new(), |tty| format!("/dev/{tty}")),
            );
        }
        Opt::Critical => {
            let mut reboot_target = None;
            let mut window = None;
            for arg in &args[1..] {
                let parts: Vec<&str> = arg.split('=').collect();
                if parts.len() != 2 {
                    return Err(format!("critical: Argument '{arg}' is not supported"));
                }
                match parts[0] {
                    "target" => reboot_target = Some(parts[1].to_string()),
                    "window" => {
                        let expanded = expand_props(parts[1], env.properties, env.vendor_api_level)
                            .map_err(|_| {
                                format!("critical: Could not expand argument ': {}", parts[1])
                            })?;
                        if expanded == "off" {
                            // init returns before marking the service critical.
                            return Ok(None);
                        }
                        let minutes =
                            parse_int(&expanded, 0, i32::MAX as i64).ok_or_else(|| {
                                "critical: 'fatal_crash_window' must be an integer > 0".to_string()
                            })?;
                        window = Some(minutes);
                    }
                    _ => return Err(format!("critical: Argument '{arg}' is not supported")),
                }
            }
            let critical = service.critical.get_or_insert(Critical {
                window_minutes: 4,
                reboot_target: None,
            });
            if let Some(target) = reboot_target {
                critical.reboot_target = Some(target);
            }
            if let Some(window) = window {
                critical.window_minutes = window;
            }
        }
        Opt::Disabled => service.disabled = true,
        Opt::EnterNamespace => {
            if args[1] != "net" {
                return Err("Init only supports entering network namespaces".to_string());
            }
            if service.namespaces.enter_net.is_some() {
                return Err("Only one network namespace may be entered".to_string());
            }
            service.namespaces.new_mount = true;
            service.namespaces.enter_net = Some(args.swap_remove(2));
        }
        Opt::File => {
            let mode = match args[2].as_str() {
                "r" => FileMode::Read,
                "w" => FileMode::Write,
                "rw" => FileMode::ReadWrite,
                _ => return Err("file type must be 'r', 'w' or 'rw'".to_string()),
            };
            let name = expand_props(&args[1], env.properties, env.vendor_api_level)
                .map_err(|error| format!("Could not expand file path ': {error}"))?;
            if !name.starts_with('/') || name.contains("../") {
                return Err("file name must not be relative".to_string());
            }
            if service.files.iter().any(|file| file.name == name) {
                return Err(format!("duplicate file descriptor '{name}'"));
            }
            service.files.push(FileDescriptor { name, mode });
        }
        Opt::GentleKill => service.gentle_kill = true,
        Opt::Group => {
            let group = decode(env, &args[1])
                .map_err(|e| format!("Unable to decode GID for '{}': {e}", args[1]))?;
            service.group = Some(group);
            for name in &args[2..] {
                let gid = decode(env, name)
                    .map_err(|e| format!("Unable to decode GID for '{name}': {e}"))?;
                service.supplementary_groups.push(gid);
            }
        }
        Opt::Interface => {
            let fullname = format!("{}/{}", args[1], args[2]);
            if let Some(owner) = existing
                .iter()
                .find(|svc| svc.interfaces.contains(&fullname))
                && !service.is_override
            {
                return Err(format!(
                    "Interface '{fullname}' redefined in {} but is already defined by {}",
                    service.name, owner.name
                ));
            }
            service.interfaces.insert(fullname);
        }
        Opt::Ioprio => {
            let priority = parse_int(&args[2], 0, 7).ok_or("priority value must be range 0 - 7")?;
            service.ioprio_priority = priority as i32;
            service.ioprio_class = match args[1].as_str() {
                "rt" => IoSchedClass::Rt,
                "be" => IoSchedClass::Be,
                "idle" => IoSchedClass::Idle,
                _ => return Err("ioprio option usage: ioprio <rt|be|idle> <0-7>".to_string()),
            };
        }
        Opt::Keycodes => {
            validate_keycodes(&args, env)?;
            service.other_options.push(RawOption { args, line });
            return Ok(Some("service option 'keycodes' kept verbatim".to_string()));
        }
        Opt::Memcg => {
            let keyword = args[0].clone();
            let valid = match keyword.as_str() {
                "memcg.limit_property" => true,
                _ => parse_int(&args[1], 0, i32::MAX as i64).is_some(),
            };
            if !valid {
                let what = keyword.trim_start_matches("memcg.");
                return Err(format!("{what} value must be equal or greater than 0"));
            }
            service.other_options.push(RawOption { args, line });
            return Ok(Some(format!("service option '{keyword}' kept verbatim")));
        }
        Opt::Namespace => {
            for arg in &args[1..] {
                match arg.as_str() {
                    "pid" => {
                        service.namespaces.new_pid = true;
                        service.namespaces.new_mount = true;
                    }
                    "mnt" => service.namespaces.new_mount = true,
                    _ => return Err("namespace must be 'pid' or 'mnt'".to_string()),
                }
            }
        }
        Opt::Oneshot => service.oneshot = true,
        Opt::Onrestart => {
            args.remove(0);
            let command_line = service.onrestart.len() + 1;
            let command = CommandSpec::parse(args, command_line)
                .map_err(|e| format!("cannot add Onrestart command: {e}"))?;
            service.onrestart.push(command);
        }
        Opt::OomScoreAdjust => {
            let value = parse_int(&args[1], MIN_OOM_SCORE_ADJUST, MAX_OOM_SCORE_ADJUST)
                .ok_or_else(|| {
                    format!(
                        "oom_score_adjust value must be in range {MIN_OOM_SCORE_ADJUST} - +{MAX_OOM_SCORE_ADJUST}"
                    )
                })?;
            service.oom_score_adjust = value as i32;
        }
        Opt::Override => service.is_override = true,
        Opt::Priority => {
            service.priority = 0;
            let value = parse_int(&args[1], ANDROID_PRIORITY_HIGHEST, ANDROID_PRIORITY_LOWEST)
                .ok_or_else(|| {
                    format!(
                        "process priority value must be range {ANDROID_PRIORITY_HIGHEST} - {ANDROID_PRIORITY_LOWEST}"
                    )
                })?;
            service.priority = value as i32;
        }
        Opt::RebootOnFailure => {
            if service.reboot_on_failure.is_some() {
                return Err("Only one reboot_on_failure command may be specified".to_string());
            }
            if !args[1].starts_with("shutdown") && !args[1].starts_with("reboot") {
                return Err(
                    "reboot_on_failure commands must begin with either 'shutdown' or 'reboot'"
                        .to_string(),
                );
            }
            service.reboot_on_failure = Some(args.swap_remove(1));
        }
        Opt::RestartPeriod => {
            let period = parse_int(&args[1], 0, i32::MAX as i64)
                .ok_or("restart_period value must be an integer >= 0")?;
            service.restart_period = period as u64;
        }
        Opt::Rlimit => service.rlimits.push(parse_rlimit(&args)?),
        Opt::Seclabel => service.seclabel = Some(args.swap_remove(1)),
        Opt::Setenv => {
            let value = args.pop().expect("arity checked");
            let name = args.pop().expect("arity checked");
            service.environment.push((name, value));
        }
        Opt::SharedKallsyms => {
            service.other_options.push(RawOption { args, line });
            return Ok(Some(
                "service option 'shared_kallsyms' kept verbatim".to_string(),
            ));
        }
        Opt::Shutdown => {
            if args[1] != "critical" {
                return Err("Invalid shutdown option".to_string());
            }
            service.shutdown_critical = true;
        }
        Opt::Sigstop => service.sigstop = true,
        Opt::Socket => {
            let socket = parse_socket(&args, env)?;
            if service
                .sockets
                .iter()
                .any(|other| other.name == socket.name)
            {
                return Err(format!("duplicate socket descriptor '{}'", socket.name));
            }
            service.sockets.push(socket);
        }
        Opt::StdioToKmsg => {
            if service.console.is_some() {
                return Err("'stdio_to_kmsg' and 'console' are mutually exclusive".to_string());
            }
            service.stdio_to_kmsg = true;
        }
        Opt::TaskProfiles => {
            service.task_profiles.extend(args.drain(1..));
        }
        Opt::TimeoutPeriod => {
            let period = parse_int(&args[1], 1, i32::MAX as i64)
                .ok_or("timeout_period value must be an integer >= 1")?;
            service.timeout_period = Some(period as u64);
        }
        Opt::Updatable => service.updatable = true,
        Opt::User => {
            let user = decode(env, &args[1])
                .map_err(|e| format!("Unable to find UID for '{}': {e}", args[1]))?;
            service.user = Some(user);
        }
        Opt::Writepid => {
            let mut warnings = Vec::new();
            let mut files = Vec::new();
            for file in args.drain(1..) {
                if let Some(profile) = convert_task_file_to_profile(&file) {
                    warnings.push(format!(
                        "'writepid {file}' is converted into 'task_profiles {profile}' for service {}",
                        service.name
                    ));
                    service.task_profiles.push(profile.to_string());
                } else {
                    files.push(file);
                }
            }
            service.writepid_files = files;
            if !warnings.is_empty() {
                return Ok(Some(warnings.join("; ")));
            }
        }
    }
    Ok(None)
}

/// `ConvertTaskFileToProfile`.
fn convert_task_file_to_profile(file: &str) -> Option<&'static str> {
    match file {
        "/dev/cpuset/camera-daemon/tasks" => Some("CameraServiceCapacity"),
        "/dev/cpuset/foreground/tasks" => Some("ProcessCapacityHigh"),
        "/dev/cpuset/system-background/tasks" => Some("ServiceCapacityLow"),
        "/dev/blkio/background/tasks" => Some("LowIoPriority"),
        _ => None,
    }
}

/// `ServiceParser::ParseKeycodes` validation (the option itself is kept raw).
fn validate_keycodes(args: &[String], env: &ParseEnv<'_>) -> Result<(), String> {
    let mut codes: Vec<String> = args[1..].to_vec();
    if args.len() == 2 && args[1].starts_with('$') {
        let expanded = expand_props(&args[1], env.properties, env.vendor_api_level)?;
        if expanded == "none" {
            return Ok(());
        }
        codes = expanded.split(',').map(str::to_string).collect();
    }
    let mut seen = Vec::new();
    for code in &codes {
        let value =
            parse_int(code, 0, KEY_MAX).ok_or_else(|| format!("invalid keycode: {code}"))?;
        if seen.contains(&value) {
            return Err(format!("duplicate keycode: {code}"));
        }
        seen.push(value);
    }
    Ok(())
}

/// `ServiceParser::ParseSocket`.
fn parse_socket(args: &[String], env: &ParseEnv<'_>) -> Result<SocketDescriptor, String> {
    let types: Vec<&str> = args[2].split('+').collect();
    let socket_type = match types[0] {
        "stream" => SocketType::Stream,
        "dgram" => SocketType::Dgram,
        "seqpacket" => SocketType::Seqpacket,
        other => {
            return Err(format!(
                "socket type must be 'dgram', 'stream' or 'seqpacket', got '{other}' instead."
            ));
        }
    };
    let mut passcred = false;
    let mut listen = false;
    for decoration in &types[1..] {
        match *decoration {
            "passcred" => passcred = true,
            "listen" => listen = true,
            other => {
                return Err(format!(
                    "Unknown socket type decoration '{other}'. Known values are ['passcred', 'listen']"
                ));
            }
        }
    }
    let perm = strtol_octal(&args[3])
        .ok_or_else(|| format!("Unable to parse permissions '{}'", args[3]))?;
    let uid = match args.get(4) {
        Some(name) => {
            Some(decode(env, name).map_err(|e| format!("Unable to find UID for '{name}': {e}"))?)
        }
        None => None,
    };
    let gid = match args.get(5) {
        Some(name) => {
            Some(decode(env, name).map_err(|e| format!("Unable to find GID for '{name}': {e}"))?)
        }
        None => None,
    };
    Ok(SocketDescriptor {
        name: args[1].clone(),
        socket_type,
        passcred,
        listen,
        perm,
        uid,
        gid,
        context: args.get(6).cloned().unwrap_or_default(),
    })
}

/// `strtol(s, &end, 8)` requiring the whole string to be consumed.
fn strtol_octal(s: &str) -> Option<u32> {
    let trimmed =
        s.trim_start_matches(|c: char| c.is_ascii() && crate::libbase::is_c_space(c as u8));
    let (negative, digits) = match trimmed.as_bytes().first() {
        Some(b'-') => (true, &trimmed[1..]),
        Some(b'+') => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    if digits.is_empty() || !digits.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
        return None;
    }
    let value = i64::from_str_radix(digits, 8).ok()?;
    let value = if negative { -value } else { value };
    Some(value as u32)
}

/// Linux `RLIMIT_*` names in rlimit_parser.cpp order.
const RLIMIT_NAMES: &[(&str, u32)] = &[
    ("cpu", 0),
    ("fsize", 1),
    ("data", 2),
    ("stack", 3),
    ("core", 4),
    ("rss", 5),
    ("nproc", 6),
    ("nofile", 7),
    ("memlock", 8),
    ("as", 9),
    ("locks", 10),
    ("sigpending", 11),
    ("msgqueue", 12),
    ("nice", 13),
    ("rtprio", 14),
    ("rttime", 15),
];
/// `RLIM_NLIMITS` on Linux.
const RLIM_NLIMITS: i64 = 16;

/// `ParseRlimit` (system/core/init/rlimit_parser.cpp); shared with the
/// `setrlimit` builtin.
pub fn parse_rlimit(args: &[String]) -> Result<Rlimit, String> {
    let resource = if let Some(number) = parse_int(&args[1], i32::MIN as i64, i32::MAX as i64) {
        if number >= RLIM_NLIMITS {
            return Err(format!(
                "Resource '{}' over the maximum resource value '{RLIM_NLIMITS}'",
                args[1]
            ));
        }
        if number < 0 {
            return Err(format!(
                "Resource '{}' below the minimum resource value '0'",
                args[1]
            ));
        }
        number as u32
    } else {
        let name = args[1]
            .strip_prefix("RLIM_")
            .or_else(|| args[1].strip_prefix("RLIMIT_"))
            .unwrap_or(&args[1]);
        RLIMIT_NAMES
            .iter()
            .find(|(text, _)| text.eq_ignore_ascii_case(name))
            .map(|(_, resource)| *resource)
            .ok_or_else(|| format!("Could not parse resource '{}'", args[1]))?
    };
    let limit = |text: &str, what: &str| -> Result<u64, String> {
        if text == "-1" || text == "unlimited" {
            return Ok(u64::MAX);
        }
        parse_uint(text, u64::MAX).ok_or_else(|| format!("Could not parse {what} limit '{text}'"))
    };
    Ok(Rlimit {
        resource,
        soft: limit(&args[2], "soft")?,
        hard: limit(&args[3], "hard")?,
    })
}

/// `ServiceParser::EndSection` checks that do not involve the service list.
pub(crate) fn end_service_checks(
    service: &Service,
    env: &ParseEnv<'_>,
) -> Result<Option<String>, String> {
    let mut warning = None;
    if service.user.is_none() {
        let vendor_api_level = parse_int(
            &env.properties.property_or("ro.vendor.api_level", "0"),
            i32::MIN as i64,
            i32::MAX as i64,
        )
        .unwrap_or(0);
        if vendor_api_level > 202404 {
            return Err(format!(
                "No user specified for service '{}', so it would have been root.",
                service.name
            ));
        }
        warning = Some(format!(
            "No user specified for service '{}', so it is root.",
            service.name
        ));
    }
    if env.vendor_api_level >= ANDROID_API_R && service.critical.is_some() && service.oneshot {
        return Err(format!(
            "service '{}' can't be both critical and oneshot",
            service.name
        ));
    }
    Ok(warning)
}
