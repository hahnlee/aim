//! init's builtin commands (`GetBuiltinFunctionMap`, system/core/init/builtins.cpp).

use super::keywords::{self, UNBOUNDED};

/// Every builtin command keyword in Android 16 init.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Builtin {
    Bootchart,
    Chmod,
    Chown,
    ClassReset,
    ClassRestart,
    ClassStart,
    ClassStop,
    Copy,
    CopyPerLine,
    Domainname,
    Enable,
    Exec,
    ExecBackground,
    ExecStart,
    Export,
    Hostname,
    Ifup,
    InitUser0,
    Insmod,
    Installkey,
    InterfaceRestart,
    InterfaceStart,
    InterfaceStop,
    LoadExports,
    LoadPersistProps,
    LoadSystemProps,
    Loglevel,
    MarkPostData,
    Mkdir,
    MountAll,
    Mount,
    PerformApexConfig,
    Umount,
    UmountAll,
    UpdateLinkerConfig,
    Readahead,
    Restart,
    Restorecon,
    RestoreconRecursive,
    Rm,
    Rmdir,
    Setprop,
    Setrlimit,
    Start,
    Stop,
    SwaponAll,
    Swapoff,
    EnterDefaultMountNs,
    Symlink,
    Sysclktz,
    Trigger,
    VerityUpdateState,
    Wait,
    WaitForProp,
    Write,
}

/// `(keyword, min_args, max_args, (builtin, run_in_subcontext))`, verbatim
/// from the Android 16 builtin function map.
pub(crate) const BUILTIN_TABLE: &[(&str, usize, usize, (Builtin, bool))] = &[
    ("bootchart", 1, 1, (Builtin::Bootchart, false)),
    ("chmod", 2, 2, (Builtin::Chmod, true)),
    ("chown", 2, 3, (Builtin::Chown, true)),
    ("class_reset", 1, 1, (Builtin::ClassReset, false)),
    ("class_restart", 1, 2, (Builtin::ClassRestart, false)),
    ("class_start", 1, 1, (Builtin::ClassStart, false)),
    ("class_stop", 1, 1, (Builtin::ClassStop, false)),
    ("copy", 2, 2, (Builtin::Copy, true)),
    ("copy_per_line", 2, 2, (Builtin::CopyPerLine, true)),
    ("domainname", 1, 1, (Builtin::Domainname, true)),
    ("enable", 1, 1, (Builtin::Enable, false)),
    ("exec", 1, UNBOUNDED, (Builtin::Exec, false)),
    (
        "exec_background",
        1,
        UNBOUNDED,
        (Builtin::ExecBackground, false),
    ),
    ("exec_start", 1, 1, (Builtin::ExecStart, false)),
    ("export", 2, 2, (Builtin::Export, false)),
    ("hostname", 1, 1, (Builtin::Hostname, true)),
    ("ifup", 1, 1, (Builtin::Ifup, true)),
    ("init_user0", 0, 0, (Builtin::InitUser0, false)),
    ("insmod", 1, UNBOUNDED, (Builtin::Insmod, true)),
    ("installkey", 1, 1, (Builtin::Installkey, false)),
    (
        "interface_restart",
        1,
        1,
        (Builtin::InterfaceRestart, false),
    ),
    ("interface_start", 1, 1, (Builtin::InterfaceStart, false)),
    ("interface_stop", 1, 1, (Builtin::InterfaceStop, false)),
    ("load_exports", 1, 1, (Builtin::LoadExports, false)),
    (
        "load_persist_props",
        0,
        0,
        (Builtin::LoadPersistProps, false),
    ),
    ("load_system_props", 0, 0, (Builtin::LoadSystemProps, false)),
    ("loglevel", 1, 1, (Builtin::Loglevel, false)),
    ("mark_post_data", 0, 0, (Builtin::MarkPostData, false)),
    ("mkdir", 1, 6, (Builtin::Mkdir, true)),
    ("mount_all", 0, UNBOUNDED, (Builtin::MountAll, false)),
    ("mount", 3, UNBOUNDED, (Builtin::Mount, false)),
    (
        "perform_apex_config",
        0,
        1,
        (Builtin::PerformApexConfig, false),
    ),
    ("umount", 1, 1, (Builtin::Umount, false)),
    ("umount_all", 0, 1, (Builtin::UmountAll, false)),
    (
        "update_linker_config",
        0,
        0,
        (Builtin::UpdateLinkerConfig, false),
    ),
    ("readahead", 1, 2, (Builtin::Readahead, true)),
    ("restart", 1, 2, (Builtin::Restart, false)),
    ("restorecon", 1, UNBOUNDED, (Builtin::Restorecon, true)),
    (
        "restorecon_recursive",
        1,
        UNBOUNDED,
        (Builtin::RestoreconRecursive, true),
    ),
    ("rm", 1, 1, (Builtin::Rm, true)),
    ("rmdir", 1, 1, (Builtin::Rmdir, true)),
    ("setprop", 2, 2, (Builtin::Setprop, true)),
    ("setrlimit", 3, 3, (Builtin::Setrlimit, false)),
    ("start", 1, 1, (Builtin::Start, false)),
    ("stop", 1, 1, (Builtin::Stop, false)),
    ("swapon_all", 0, 1, (Builtin::SwaponAll, false)),
    ("swapoff", 1, 1, (Builtin::Swapoff, false)),
    (
        "enter_default_mount_ns",
        0,
        0,
        (Builtin::EnterDefaultMountNs, false),
    ),
    ("symlink", 2, 2, (Builtin::Symlink, true)),
    ("sysclktz", 1, 1, (Builtin::Sysclktz, false)),
    ("trigger", 1, 1, (Builtin::Trigger, false)),
    (
        "verity_update_state",
        0,
        0,
        (Builtin::VerityUpdateState, false),
    ),
    ("wait", 1, 2, (Builtin::Wait, true)),
    ("wait_for_prop", 2, 2, (Builtin::WaitForProp, false)),
    ("write", 2, 2, (Builtin::Write, true)),
];

impl Builtin {
    pub fn keyword(self) -> &'static str {
        BUILTIN_TABLE
            .iter()
            .find(|(_, _, _, (builtin, _))| *builtin == self)
            .map(|(name, ..)| *name)
            .expect("every builtin is in the table")
    }

    /// Whether init runs it in the vendor_init subcontext when the action
    /// comes from a vendor/odm file.
    pub fn runs_in_subcontext(self) -> bool {
        BUILTIN_TABLE
            .iter()
            .find(|(_, _, _, (builtin, _))| *builtin == self)
            .map(|(_, _, _, (_, subcontext))| *subcontext)
            .unwrap_or(false)
    }
}

/// One command line of an action (or of a service's `onrestart`), as parsed:
/// arguments are kept unexpanded because init expands `${...}` only when the
/// command runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    pub builtin: Builtin,
    /// `args[0]` is the keyword.
    pub args: Vec<String>,
    pub line: usize,
}

impl CommandSpec {
    /// `Action::AddCommand`: looks the keyword up and checks its arity.
    pub fn parse(args: Vec<String>, line: usize) -> Result<Self, String> {
        let (builtin, _) = *keywords::find(BUILTIN_TABLE, &args)?;
        Ok(Self {
            builtin,
            args,
            line,
        })
    }

    /// `Command::BuildCommandString`.
    pub fn command_string(&self) -> String {
        self.args.join(" ")
    }
}
