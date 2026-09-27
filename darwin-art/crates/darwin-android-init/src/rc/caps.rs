//! Capability names accepted by the `capabilities` option
//! (system/core/init/capabilities.cpp `cap_map`), in Linux numbering order.

/// `CAP_<name>` without the prefix; the index is the Linux capability number.
pub const CAPABILITY_NAMES: &[&str] = &[
    "CHOWN",
    "DAC_OVERRIDE",
    "DAC_READ_SEARCH",
    "FOWNER",
    "FSETID",
    "KILL",
    "SETGID",
    "SETUID",
    "SETPCAP",
    "LINUX_IMMUTABLE",
    "NET_BIND_SERVICE",
    "NET_BROADCAST",
    "NET_ADMIN",
    "NET_RAW",
    "IPC_LOCK",
    "IPC_OWNER",
    "SYS_MODULE",
    "SYS_RAWIO",
    "SYS_CHROOT",
    "SYS_PTRACE",
    "SYS_PACCT",
    "SYS_ADMIN",
    "SYS_BOOT",
    "SYS_NICE",
    "SYS_RESOURCE",
    "SYS_TIME",
    "SYS_TTY_CONFIG",
    "MKNOD",
    "LEASE",
    "AUDIT_WRITE",
    "AUDIT_CONTROL",
    "SETFCAP",
    "MAC_OVERRIDE",
    "MAC_ADMIN",
    "SYSLOG",
    "WAKE_ALARM",
    "BLOCK_SUSPEND",
    "AUDIT_READ",
    "PERFMON",
    "BPF",
    "CHECKPOINT_RESTORE",
];

/// `LookupCap`: exact (case-sensitive) match.
pub fn lookup_cap(name: &str) -> Option<u32> {
    CAPABILITY_NAMES
        .iter()
        .position(|cap| *cap == name)
        .map(|index| index as u32)
}

/// Names of the capabilities set in `mask`.
pub fn capability_names(mask: u64) -> Vec<&'static str> {
    CAPABILITY_NAMES
        .iter()
        .enumerate()
        .filter(|(index, _)| mask & (1 << index) != 0)
        .map(|(_, name)| *name)
        .collect()
}
