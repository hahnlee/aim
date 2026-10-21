//! The cgroup v1 hierarchies Android mounts (`cgroups.json`, libprocessgroup's
//! `MountV1CgroupController`), as the kernel the image targets
//! (android16-6.12 GKI) provides them: `blkio` (blk-cgroup with blk-ioprio),
//! `cpu` (group scheduling and utilization clamping) and `cpuset`, mounted
//! the legacy way (`noprefix`). The kernel there also has CFS bandwidth
//! control, which Darwin cannot enforce on a group, so its files are not
//! offered, as on a kernel without CONFIG_CFS_BANDWIDTH.
//!
//! A hierarchy is a host directory: each group a directory with its control
//! files, which hold what reading them shows (`kernel/cgroup/cgroup-v1.c`,
//! `kernel/sched/core.c`, `kernel/cgroup/cpuset-v1.c`, `block/blk-ioprio.c`
//! and `block/blk-cgroup.c`). guest-init (init's `SetupCgroups` and its
//! commands) and the syscall layer (the guest's) both make groups and write
//! control files through [`populate`] and [`write`]. Which group each task
//! is in is boot-wide kernel state the syscall layer keeps next to the
//! hierarchy ([`state_path`], [`Membership`]).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Linux errno values the control files answer with.
pub const EPERM: i32 = 1;
pub const ESRCH: i32 = 3;
pub const EIO: i32 = 5;
pub const E2BIG: i32 = 7;
pub const EACCES: i32 = 13;
pub const EBUSY: i32 = 16;
pub const EINVAL: i32 = 22;
pub const ENOSPC: i32 = 28;
pub const ERANGE: i32 = 34;

/// `CONFIG_NR_CPUS` of the GKI kernel: the bits a cpu list may name.
const NR_CPUS: usize = 32;
/// No NUMA: one memory node (`MAX_NUMNODES`).
const MAX_NUMNODES: usize = 1;
/// The longest write a control file takes (`PAGE_SIZE`, `PATH_MAX - 1` for
/// `release_agent`).
const MAX_WRITE: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Controller {
    Blkio,
    Cpu,
    Cpuset,
}

impl Controller {
    pub const ALL: [Controller; 3] = [Controller::Blkio, Controller::Cpu, Controller::Cpuset];

    pub fn name(self) -> &'static str {
        match self {
            Controller::Blkio => "blkio",
            Controller::Cpu => "cpu",
            Controller::Cpuset => "cpuset",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }

    /// The hierarchy id `/proc/<pid>/cgroup` shows: the order of the mounts,
    /// which libprocessgroup makes in controller name order.
    pub fn hierarchy_id(self) -> u32 {
        self as u32 + 1
    }

    /// The path map kind (and mount `fstype`) of the hierarchy.
    pub fn map_kind(self) -> String {
        format!("cgroup:{}", self.name())
    }

    pub fn from_map_kind(kind: &str) -> Option<Self> {
        Self::from_name(kind.strip_prefix("cgroup:")?)
    }

    /// The mount options `/proc/mounts` shows after the common ones
    /// (`cgroup1_show_options`): the controller, and for the legacy cpuset
    /// filesystem `noprefix` and its release agent.
    pub fn mount_options(self) -> &'static str {
        match self {
            Controller::Blkio => "blkio",
            Controller::Cpu => "cpu",
            Controller::Cpuset => "cpuset,noprefix,release_agent=/sbin/cpuset_release_agent",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Controller::Blkio => "blkio.",
            Controller::Cpu => "cpu.",
            Controller::Cpuset => "",
        }
    }
}

/// What a control file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum File {
    Tasks,
    Procs,
    CloneChildren,
    NotifyOnRelease,
    ReleaseAgent,
    SaneBehavior,
    Shares,
    Idle,
    UclampMin,
    UclampMax,
    UclampLatencySensitive,
    Cpus,
    Mems,
    EffectiveCpus,
    EffectiveMems,
    CpuExclusive,
    MemExclusive,
    MemHardwall,
    SchedLoadBalance,
    SchedRelaxDomainLevel,
    MemoryMigrate,
    MemoryPressure,
    MemorySpreadPage,
    MemorySpreadSlab,
    MemoryPressureEnabled,
    PrioClass,
    ResetStats,
}

/// Where a file exists (`CFTYPE_ONLY_ON_ROOT`, `CFTYPE_NOT_ON_ROOT`).
#[derive(Clone, Copy, PartialEq)]
enum On {
    All,
    Root,
    Children,
}

/// The access a file has: `cgroup_file_mode`: readable 0444, writable 0200.
#[derive(Clone, Copy, PartialEq)]
enum Access {
    ReadWrite,
    Read,
    Write,
}

struct Spec {
    name: &'static str,
    file: File,
    on: On,
    access: Access,
}

const fn spec(name: &'static str, file: File, on: On, access: Access) -> Spec {
    Spec {
        name,
        file,
        on,
        access,
    }
}

/// `cgroup1_base_files`.
const BASE: &[Spec] = &[
    spec("cgroup.procs", File::Procs, On::All, Access::ReadWrite),
    spec(
        "cgroup.clone_children",
        File::CloneChildren,
        On::All,
        Access::ReadWrite,
    ),
    spec(
        "cgroup.sane_behavior",
        File::SaneBehavior,
        On::Root,
        Access::Read,
    ),
    spec("tasks", File::Tasks, On::All, Access::ReadWrite),
    spec(
        "notify_on_release",
        File::NotifyOnRelease,
        On::All,
        Access::ReadWrite,
    ),
    spec(
        "release_agent",
        File::ReleaseAgent,
        On::Root,
        Access::ReadWrite,
    ),
];

/// `blkcg_legacy_files` and blk-ioprio's `ioprio_legacy_files`.
const BLKIO: &[Spec] = &[
    spec("reset_stats", File::ResetStats, On::All, Access::Write),
    spec("prio.class", File::PrioClass, On::All, Access::ReadWrite),
];

/// `cpu_legacy_files` for group scheduling and utilization clamping.
const CPU: &[Spec] = &[
    spec("shares", File::Shares, On::All, Access::ReadWrite),
    spec("idle", File::Idle, On::All, Access::ReadWrite),
    spec(
        "uclamp.min",
        File::UclampMin,
        On::Children,
        Access::ReadWrite,
    ),
    spec(
        "uclamp.max",
        File::UclampMax,
        On::Children,
        Access::ReadWrite,
    ),
    spec(
        "uclamp.latency_sensitive",
        File::UclampLatencySensitive,
        On::Children,
        Access::ReadWrite,
    ),
];

/// `cpuset1_files`.
const CPUSET: &[Spec] = &[
    spec("cpus", File::Cpus, On::All, Access::ReadWrite),
    spec("mems", File::Mems, On::All, Access::ReadWrite),
    spec("effective_cpus", File::EffectiveCpus, On::All, Access::Read),
    spec("effective_mems", File::EffectiveMems, On::All, Access::Read),
    spec(
        "cpu_exclusive",
        File::CpuExclusive,
        On::All,
        Access::ReadWrite,
    ),
    spec(
        "mem_exclusive",
        File::MemExclusive,
        On::All,
        Access::ReadWrite,
    ),
    spec(
        "mem_hardwall",
        File::MemHardwall,
        On::All,
        Access::ReadWrite,
    ),
    spec(
        "sched_load_balance",
        File::SchedLoadBalance,
        On::All,
        Access::ReadWrite,
    ),
    spec(
        "sched_relax_domain_level",
        File::SchedRelaxDomainLevel,
        On::All,
        Access::ReadWrite,
    ),
    spec(
        "memory_migrate",
        File::MemoryMigrate,
        On::All,
        Access::ReadWrite,
    ),
    spec(
        "memory_pressure",
        File::MemoryPressure,
        On::All,
        Access::Read,
    ),
    spec(
        "memory_spread_page",
        File::MemorySpreadPage,
        On::All,
        Access::ReadWrite,
    ),
    spec(
        "memory_spread_slab",
        File::MemorySpreadSlab,
        On::All,
        Access::ReadWrite,
    ),
    spec(
        "memory_pressure_enabled",
        File::MemoryPressureEnabled,
        On::Root,
        Access::ReadWrite,
    ),
];

fn controller_specs(c: Controller) -> &'static [Spec] {
    match c {
        Controller::Blkio => BLKIO,
        Controller::Cpu => CPU,
        Controller::Cpuset => CPUSET,
    }
}

/// The control files of a group, with their names and modes.
pub fn files(c: Controller, root: bool) -> Vec<(String, File, u32)> {
    let on = |spec: &Spec| match spec.on {
        On::All => true,
        On::Root => root,
        On::Children => !root,
    };
    let mode = |spec: &Spec| match spec.access {
        Access::ReadWrite => 0o644,
        Access::Read => 0o444,
        Access::Write => 0o200,
    };
    let base = BASE
        .iter()
        .filter(|s| on(s))
        .map(|s| (s.name.to_string(), s.file, mode(s)));
    let own = controller_specs(c)
        .iter()
        .filter(|s| on(s))
        .map(|s| (format!("{}{}", c.prefix(), s.name), s.file, mode(s)));
    base.chain(own).collect()
}

/// The control file `name` of a group.
pub fn file(c: Controller, root: bool, name: &str) -> Option<File> {
    files(c, root)
        .into_iter()
        .find(|(n, ..)| n == name)
        .map(|(_, f, _)| f)
}

/// The cpus the guest has online (`cpu_online_mask`), as a list.
pub fn online_cpus() -> String {
    let n = std::thread::available_parallelism().map_or(1, |n| n.get());
    format_list(&(0..n.min(NR_CPUS)).collect::<Vec<_>>())
}

/// A bitmap as `bitmap_print_to_pagebuf` lists it: `0-3,6`.
pub fn format_list(bits: &[usize]) -> String {
    let mut bits = bits.to_vec();
    bits.sort_unstable();
    bits.dedup();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bits.len() {
        let start = bits[i];
        while i + 1 < bits.len() && bits[i + 1] == bits[i] + 1 {
            i += 1;
        }
        out.push(if bits[i] == start {
            start.to_string()
        } else {
            format!("{start}-{}", bits[i])
        });
        i += 1;
    }
    out.join(",")
}

/// `bitmap_parselist` over `nbits` bits: `a`, `a-b` and `a-b:used/group`,
/// comma separated.
pub fn parse_list(s: &str, nbits: usize) -> Result<Vec<usize>, i32> {
    let mut bits = Vec::new();
    if s.is_empty() {
        return Ok(bits);
    }
    let num = |t: &str| t.parse::<usize>().map_err(|_| EINVAL);
    for chunk in s.split(',') {
        let (range, group) = match chunk.split_once(':') {
            Some((r, g)) => (r, Some(g)),
            None => (chunk, None),
        };
        let (a, b) = match range.split_once('-') {
            Some((a, b)) => (num(a)?, num(b)?),
            None => {
                let a = num(range)?;
                (a, a)
            }
        };
        let (used, step) = match group {
            Some(g) => {
                let (u, g) = g.split_once('/').ok_or(EINVAL)?;
                (num(u)?, num(g)?)
            }
            None => (1, 1),
        };
        if a > b || used == 0 || step == 0 || used > step {
            return Err(EINVAL);
        }
        if b >= nbits {
            return Err(ERANGE);
        }
        let mut start = a;
        while start <= b {
            bits.extend(start..(start + used).min(b + 1));
            start += step;
        }
    }
    bits.sort_unstable();
    bits.dedup();
    Ok(bits)
}

/// `kstrtoull(s, 0)` / `kstrtoll(s, 0)`: a number in base 0, at most one
/// trailing newline.
fn parse_int(s: &str, signed: bool) -> Result<i128, i32> {
    let s = s.strip_suffix('\n').unwrap_or(s);
    let (neg, s) = match s.as_bytes().first() {
        Some(b'-') if signed => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let (radix, digits) = if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        (16, h)
    } else if s.len() > 1 && s.starts_with('0') {
        (8, &s[1..])
    } else {
        (10, s)
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return Err(EINVAL);
    }
    let v = u128::from_str_radix(digits, radix).map_err(|_| ERANGE)?;
    let v = if neg { -(v as i128) } else { v as i128 };
    let (lo, hi) = if signed {
        (i64::MIN as i128, i64::MAX as i128)
    } else {
        (0, u64::MAX as i128)
    };
    if v < lo || v > hi {
        return Err(ERANGE);
    }
    Ok(v)
}

/// The default content of a control file of a new group (`parent` is its
/// parent's directory, None for the root).
fn default_value(c: Controller, f: File, parent: Option<&Path>) -> String {
    let parent_value = |name: &str| {
        parent
            .and_then(|p| fs::read_to_string(p.join(name)).ok())
            .unwrap_or_default()
    };
    let inherited = |name: &str| {
        if parent_value(name).trim() == "1" {
            "1\n"
        } else {
            "0\n"
        }
    };
    let cloned = parent.is_some() && parent_value("cgroup.clone_children").trim() == "1";
    match f {
        File::Tasks | File::Procs | File::ResetStats => String::new(),
        File::CloneChildren => inherited("cgroup.clone_children").into(),
        File::NotifyOnRelease => inherited("notify_on_release").into(),
        File::ReleaseAgent if c == Controller::Cpuset => "/sbin/cpuset_release_agent\n".into(),
        File::ReleaseAgent => "\n".into(),
        File::SaneBehavior => "0\n".into(),
        File::Shares => "1024\n".into(),
        File::Idle | File::UclampLatencySensitive => "0\n".into(),
        File::UclampMin => "0.00\n".into(),
        File::UclampMax => "max\n".into(),
        File::Cpus | File::EffectiveCpus => match parent {
            None => format!("{}\n", online_cpus()),
            Some(_) if cloned => parent_value("cpus"),
            Some(_) => "\n".into(),
        },
        File::Mems | File::EffectiveMems => match parent {
            None => "0\n".into(),
            Some(_) if cloned => parent_value("mems"),
            Some(_) => "\n".into(),
        },
        // top_cpuset: CS_CPU_EXCLUSIVE | CS_MEM_EXCLUSIVE.
        File::CpuExclusive | File::MemExclusive if parent.is_none() => "1\n".into(),
        File::CpuExclusive | File::MemExclusive | File::MemHardwall | File::MemoryMigrate => {
            "0\n".into()
        }
        File::SchedLoadBalance => "1\n".into(),
        File::SchedRelaxDomainLevel => "-1\n".into(),
        File::MemoryPressure | File::MemoryPressureEnabled => "0\n".into(),
        File::MemorySpreadPage => inherited("memory_spread_page").into(),
        File::MemorySpreadSlab => inherited("memory_spread_slab").into(),
        File::PrioClass => "no-change\n".into(),
    }
}

/// Fills a new group's directory `dir` with its control files
/// (`cgroup_populate_dir`, with what `css_online` copies from the parent).
/// Returns the files made, with their modes.
pub fn populate(
    c: Controller,
    dir: &Path,
    parent: Option<&Path>,
) -> io::Result<Vec<(PathBuf, u32)>> {
    let mut made = Vec::new();
    for (name, f, mode) in files(c, parent.is_none()) {
        let path = dir.join(&name);
        fs::write(&path, default_value(c, f, parent))?;
        made.push((path, mode));
    }
    Ok(made)
}

/// Whether `dir`, a group of a hierarchy rooted at `root`, is the root.
fn parent_of<'a>(dir: &'a Path, root: &Path) -> Option<&'a Path> {
    (dir != root).then(|| dir.parent()).flatten()
}

fn read_flag(dir: &Path, name: &str) -> bool {
    fs::read_to_string(dir.join(name)).is_ok_and(|v| v.trim() == "1")
}

fn read_list(dir: &Path, name: &str) -> Vec<usize> {
    fs::read_to_string(dir.join(name))
        .ok()
        .and_then(|v| parse_list(v.trim(), NR_CPUS).ok())
        .unwrap_or_default()
}

fn subset(a: &[usize], b: &[usize]) -> bool {
    a.iter().all(|x| b.contains(x))
}

/// The child groups of `dir`.
pub fn children(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default()
}

/// A cpuset as `is_cpuset_subset` compares it.
struct Cpuset {
    cpus: Vec<usize>,
    mems: Vec<usize>,
    cpu_exclusive: bool,
    mem_exclusive: bool,
}

impl Cpuset {
    fn read(dir: &Path) -> Self {
        Cpuset {
            cpus: read_list(dir, "cpus"),
            mems: read_list(dir, "mems"),
            cpu_exclusive: read_flag(dir, "cpu_exclusive"),
            mem_exclusive: read_flag(dir, "mem_exclusive"),
        }
    }

    fn subset_of(&self, q: &Cpuset) -> bool {
        subset(&self.cpus, &q.cpus)
            && subset(&self.mems, &q.mems)
            && self.cpu_exclusive <= q.cpu_exclusive
            && self.mem_exclusive <= q.mem_exclusive
    }
}

/// `validate_change` with `cpuset1_validate_change` for a v1 cpuset at
/// `dir` changed to `trial`.
fn validate_cpuset(dir: &Path, root: &Path, trial: &Cpuset, populated: bool) -> Result<(), i32> {
    // Each child must be a subset of us.
    if children(dir)
        .iter()
        .any(|child| !Cpuset::read(child).subset_of(trial))
    {
        return Err(EBUSY);
    }
    let Some(parent) = parent_of(dir, root) else {
        return Ok(());
    };
    // We must be a subset of our parent.
    if !trial.subset_of(&Cpuset::read(parent)) {
        return Err(EACCES);
    }
    // A cpuset with tasks keeps cpus and mems.
    let current = Cpuset::read(dir);
    if populated
        && (!current.cpus.is_empty() && trial.cpus.is_empty()
            || !current.mems.is_empty() && trial.mems.is_empty())
    {
        return Err(ENOSPC);
    }
    // Exclusive siblings must not overlap.
    for sibling in children(parent).iter().filter(|s| s.as_path() != dir) {
        let s = Cpuset::read(sibling);
        if (trial.cpu_exclusive || s.cpu_exclusive) && s.cpus.iter().any(|c| trial.cpus.contains(c))
        {
            return Err(EINVAL);
        }
        if (trial.mem_exclusive || s.mem_exclusive) && s.mems.iter().any(|m| trial.mems.contains(m))
        {
            return Err(EINVAL);
        }
    }
    Ok(())
}

fn store(path: &Path, value: &str) -> Result<(), i32> {
    fs::write(path, value).map_err(|_| EIO)
}

/// `cgroup_parse_float(s, 2)`: a decimal with up to two places, in
/// hundredths.
fn parse_hundredths(s: &str) -> Result<i64, i32> {
    let (whole, frac) = match s.split_once('.') {
        Some((w, f)) => (w, f),
        None => (s, ""),
    };
    let whole: i64 = whole.parse().map_err(|_| EINVAL)?;
    let digits: String = frac.chars().take_while(|c| c.is_ascii_digit()).collect();
    let frac = match digits.len() {
        0 => 0,
        1 => digits.parse::<i64>().map_err(|_| EINVAL)? * 10,
        2 => digits.parse::<i64>().map_err(|_| EINVAL)?,
        n => {
            let v: i64 = digits.parse().map_err(|_| ERANGE)?;
            let p = 10i64.checked_pow(n as u32 - 2).ok_or(ERANGE)?;
            (v + p / 2) / p
        }
    };
    whole
        .checked_mul(100)
        .and_then(|w| w.checked_add(frac))
        .ok_or(ERANGE)
}

/// `cpu_uclamp_write` then `cpu_uclamp_print`: `max`, or the percentage
/// asked, shown as `max` when it rounds to the full capacity (1024).
fn uclamp_value(input: &str) -> Result<String, i32> {
    let input = input.trim();
    if input == "max" {
        return Ok("max\n".into());
    }
    let percent = parse_hundredths(input)?;
    if !(0..=10000).contains(&percent) {
        return Err(ERANGE);
    }
    let util = ((percent << 10) + 5000) / 10000;
    Ok(if util == 1024 {
        "max\n".into()
    } else {
        format!("{}.{:02}\n", percent / 100, percent % 100)
    })
}

/// A write of `input` to control file `f` of the group at `dir` in the
/// hierarchy rooted at `root`, at file offset `offset`: the new content is
/// stored (`populated` tells whether the group has tasks). Task moves
/// (`tasks`, `cgroup.procs`) are the syscall layer's, and so is the
/// capability `release_agent` needs (CAP_SYS_ADMIN).
pub fn write(
    f: File,
    dir: &Path,
    root: &Path,
    input: &[u8],
    offset: u64,
    populated: bool,
) -> Result<(), i32> {
    if input.len() > MAX_WRITE {
        return Err(E2BIG);
    }
    let text = std::str::from_utf8(input).map_err(|_| EINVAL)?;
    let is_root = dir == root;
    let path = |name: &str| dir.join(name);
    let u64_of = |s: &str| parse_int(s, false).map(|v| v as u64);
    let i64_of = |s: &str| parse_int(s, true).map(|v| v as i64);
    let flag = |name: &str| -> Result<(), i32> {
        let v = u64_of(text)?;
        store(&path(name), if v != 0 { "1\n" } else { "0\n" })
    };
    match f {
        File::Tasks | File::Procs => Err(EINVAL),
        File::CloneChildren => flag("cgroup.clone_children"),
        File::NotifyOnRelease => flag("notify_on_release"),
        File::ReleaseAgent => {
            let agent = text.trim();
            store(&path("release_agent"), &format!("{agent}\n"))
        }
        File::SaneBehavior | File::EffectiveCpus | File::EffectiveMems | File::MemoryPressure => {
            Err(EACCES)
        }
        File::Shares => {
            // sched_group_set_shares: not the root; MIN_SHARES to MAX_SHARES.
            let v = u64_of(text)?;
            if is_root {
                return Err(EINVAL);
            }
            store(&path("cpu.shares"), &format!("{}\n", v.clamp(2, 1 << 18)))
        }
        File::Idle => {
            let v = i64_of(text)?;
            if is_root || !(0..=1).contains(&v) {
                return Err(EINVAL);
            }
            store(&path("cpu.idle"), &format!("{v}\n"))
        }
        File::UclampMin => store(&path("cpu.uclamp.min"), &uclamp_value(text)?),
        File::UclampMax => store(&path("cpu.uclamp.max"), &uclamp_value(text)?),
        File::UclampLatencySensitive => {
            let v = u64_of(text)?;
            if v > 1 {
                return Err(EINVAL);
            }
            store(&path("cpu.uclamp.latency_sensitive"), &format!("{v}\n"))
        }
        File::Cpus | File::Mems => {
            // top_cpuset's masks track the online cpus and memory nodes.
            if is_root {
                return Err(EACCES);
            }
            let cpus = f == File::Cpus;
            let list = parse_list(text.trim(), if cpus { NR_CPUS } else { MAX_NUMNODES })?;
            let top = Cpuset::read(root);
            if !subset(&list, if cpus { &top.cpus } else { &top.mems }) {
                return Err(EINVAL);
            }
            let mut trial = Cpuset::read(dir);
            if cpus {
                trial.cpus = list.clone();
            } else {
                trial.mems = list.clone();
            }
            validate_cpuset(dir, root, &trial, populated)?;
            let value = format!("{}\n", format_list(&list));
            let (own, effective) = if cpus {
                ("cpus", "effective_cpus")
            } else {
                ("mems", "effective_mems")
            };
            store(&path(own), &value)?;
            store(&path(effective), &value)
        }
        File::CpuExclusive | File::MemExclusive => {
            let on = u64_of(text)? != 0;
            let mut trial = Cpuset::read(dir);
            let name = if f == File::CpuExclusive {
                trial.cpu_exclusive = on;
                "cpu_exclusive"
            } else {
                trial.mem_exclusive = on;
                "mem_exclusive"
            };
            validate_cpuset(dir, root, &trial, populated)?;
            store(&path(name), if on { "1\n" } else { "0\n" })
        }
        File::MemHardwall => flag("mem_hardwall"),
        File::SchedLoadBalance => flag("sched_load_balance"),
        File::MemoryMigrate => flag("memory_migrate"),
        File::MemorySpreadPage => flag("memory_spread_page"),
        File::MemorySpreadSlab => flag("memory_spread_slab"),
        File::MemoryPressureEnabled => flag("memory_pressure_enabled"),
        File::SchedRelaxDomainLevel => {
            // The scheduling domains are MC and the package: levels 0 and 1.
            let v = i64_of(text)?;
            if !(-1..=2).contains(&v) {
                return Err(EINVAL);
            }
            store(&path("sched_relax_domain_level"), &format!("{v}\n"))
        }
        File::PrioClass => {
            if offset != 0 {
                return Err(EIO);
            }
            // sysfs_match_string: one trailing newline.
            let name = text.strip_suffix('\n').unwrap_or(text);
            if ![
                "no-change",
                "promote-to-rt",
                "restrict-to-be",
                "idle",
                "none-to-rt",
            ]
            .contains(&name)
            {
                return Err(EINVAL);
            }
            store(&path("blkio.prio.class"), &format!("{name}\n"))
        }
        // There are no statistics to reset.
        File::ResetStats => u64_of(text).map(drop),
    }
}

/// A group's path in its hierarchy as `/proc/<pid>/cgroup` shows it.
pub fn group_path(dir: &Path, root: &Path) -> String {
    match dir.strip_prefix(root) {
        Ok(rest) if rest.as_os_str().is_empty() => "/".into(),
        Ok(rest) => format!("/{}", rest.display()),
        Err(_) => "/".into(),
    }
}

/// The file next to the hierarchy at `root` that holds which group each
/// task is in.
pub fn state_path(root: &Path) -> PathBuf {
    let mut name = root.file_name().unwrap_or_default().to_os_string();
    name.push(".tasks");
    root.with_file_name(name)
}

/// A process's start time, which tells it from a later one of the same pid.
pub type Start = (u64, u64);

/// Which group tasks are in: a process's threads are in its group unless a
/// thread was moved alone. Tasks not listed are in the root.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Membership {
    /// `(pid, start, group)`.
    pub processes: Vec<(i32, Start, String)>,
    /// `(tid, owner's start, group)`.
    pub threads: Vec<(i32, Start, String)>,
}

impl Membership {
    pub fn parse(text: &str) -> Self {
        let mut m = Membership::default();
        for line in text.lines() {
            let f: Vec<&str> = line.splitn(5, '\t').collect();
            let [kind, id, sec, usec, group] = f[..] else {
                continue;
            };
            let (Ok(id), Ok(sec), Ok(usec)) = (id.parse(), sec.parse(), usec.parse()) else {
                continue;
            };
            let entry = (id, (sec, usec), group.to_string());
            match kind {
                "P" => m.processes.push(entry),
                "T" => m.threads.push(entry),
                _ => {}
            }
        }
        m
    }

    pub fn text(&self) -> String {
        let line = |kind: &str, (id, (sec, usec), group): &(i32, Start, String)| {
            format!("{kind}\t{id}\t{sec}\t{usec}\t{group}\n")
        };
        self.processes
            .iter()
            .map(|e| line("P", e))
            .chain(self.threads.iter().map(|e| line("T", e)))
            .collect()
    }

    /// Whether any task is listed in `group` or below it (an rmdir's check
    /// before tasks are known to be alive).
    pub fn names(&self, group: &str) -> bool {
        let below = |g: &str| {
            g == group
                || g.strip_prefix(group)
                    .is_some_and(|rest| group == "/" || rest.starts_with('/'))
        };
        self.processes
            .iter()
            .chain(&self.threads)
            .any(|(.., g)| below(g))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aim-cgroup-v1-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lists_parse_and_print_as_bitmap_lists() {
        assert_eq!(parse_list("0-3,6", 32), Ok(vec![0, 1, 2, 3, 6]));
        assert_eq!(parse_list("0-7:2/4", 32), Ok(vec![0, 1, 4, 5]));
        assert_eq!(parse_list("3-1", 32), Err(EINVAL));
        assert_eq!(parse_list("40", 32), Err(ERANGE));
        assert_eq!(parse_list("", 32), Ok(vec![]));
        assert_eq!(format_list(&[0, 1, 2, 3, 6, 8, 9]), "0-3,6,8-9");
    }

    #[test]
    fn integers_parse_as_kstrtoull() {
        assert_eq!(parse_int("12\n", false), Ok(12));
        assert_eq!(parse_int("0x10", false), Ok(16));
        assert_eq!(parse_int("010", false), Ok(8));
        assert_eq!(parse_int("-1", false), Err(EINVAL));
        assert_eq!(parse_int("-1", true), Ok(-1));
        assert_eq!(parse_int("1 ", false), Err(EINVAL));
        assert_eq!(parse_int("99999999999999999999", false), Err(ERANGE));
    }

    #[test]
    fn files_follow_their_placement_and_access() {
        let root_files = files(Controller::Cpu, true);
        assert!(root_files.iter().any(|(n, ..)| n == "release_agent"));
        assert!(!root_files.iter().any(|(n, ..)| n == "cpu.uclamp.min"));
        let child = files(Controller::Cpu, false);
        assert!(
            child
                .iter()
                .any(|(n, _, m)| n == "cpu.uclamp.min" && *m == 0o644)
        );
        assert!(!child.iter().any(|(n, ..)| n == "cgroup.sane_behavior"));
        assert_eq!(file(Controller::Cpuset, false, "cpus"), Some(File::Cpus));
        assert_eq!(file(Controller::Cpuset, false, "cpuset.cpus"), None);
        assert!(
            files(Controller::Blkio, false)
                .iter()
                .any(|(n, _, m)| n == "blkio.reset_stats" && *m == 0o200)
        );
    }

    #[test]
    fn uclamp_values_round_as_the_kernel_prints_them() {
        assert_eq!(uclamp_value("max").unwrap(), "max\n");
        assert_eq!(uclamp_value("1").unwrap(), "1.00\n");
        assert_eq!(uclamp_value("12.345").unwrap(), "12.35\n");
        assert_eq!(uclamp_value("99.99").unwrap(), "max\n");
        assert_eq!(uclamp_value("100.01"), Err(ERANGE));
        assert_eq!(uclamp_value("x"), Err(EINVAL));
    }

    #[test]
    fn cpusets_validate_against_parent_children_and_tasks() {
        let top = root("cpuset");
        populate(Controller::Cpuset, &top, None).unwrap();
        let online = fs::read_to_string(top.join("cpus")).unwrap();
        let a = top.join("a");
        fs::create_dir(&a).unwrap();
        populate(Controller::Cpuset, &a, Some(&top)).unwrap();
        assert_eq!(fs::read_to_string(a.join("cpus")).unwrap(), "\n");
        let w = |dir: &Path, f, v: &str, populated| write(f, dir, &top, v.as_bytes(), 0, populated);
        assert_eq!(w(&top, File::Cpus, "0", false), Err(EACCES));
        w(&a, File::Mems, "0", false).unwrap();
        w(&a, File::Cpus, "0", true).unwrap();
        assert_eq!(w(&a, File::Cpus, "", true), Err(ENOSPC));
        assert_eq!(fs::read_to_string(a.join("effective_cpus")).unwrap(), "0\n");
        assert_eq!(w(&a, File::Mems, "1", false), Err(ERANGE));
        // A child must stay within its parent, the parent cover its children.
        let b = a.join("b");
        fs::create_dir(&b).unwrap();
        populate(Controller::Cpuset, &b, Some(&a)).unwrap();
        w(&b, File::Cpus, "0", false).unwrap();
        if online.trim() != "0" {
            assert_eq!(w(&b, File::Cpus, "1", false), Err(EACCES));
        }
        assert_eq!(w(&a, File::Cpus, "", false), Err(EBUSY));
        // clone_children copies the parent's masks into a new child.
        w(&a, File::CloneChildren, "1", false).unwrap();
        let c = a.join("c");
        fs::create_dir(&c).unwrap();
        populate(Controller::Cpuset, &c, Some(&a)).unwrap();
        assert_eq!(fs::read_to_string(c.join("cpus")).unwrap(), "0\n");
        assert_eq!(
            fs::read_to_string(c.join("cgroup.clone_children")).unwrap(),
            "1\n"
        );
        fs::remove_dir_all(&top).unwrap();
    }

    #[test]
    fn cpu_and_blkio_values_check_as_the_kernel() {
        let top = root("cpu");
        populate(Controller::Cpu, &top, None).unwrap();
        let g = top.join("g");
        fs::create_dir(&g).unwrap();
        populate(Controller::Cpu, &g, Some(&top)).unwrap();
        let w = |dir: &Path, f, v: &str| write(f, dir, &top, v.as_bytes(), 0, false);
        assert_eq!(w(&top, File::Shares, "512"), Err(EINVAL));
        w(&g, File::Shares, "1").unwrap();
        assert_eq!(fs::read_to_string(g.join("cpu.shares")).unwrap(), "2\n");
        assert_eq!(w(&g, File::Idle, "2"), Err(EINVAL));
        assert_eq!(w(&g, File::UclampLatencySensitive, "2"), Err(EINVAL));
        w(&g, File::UclampMin, "1").unwrap();
        assert_eq!(
            fs::read_to_string(g.join("cpu.uclamp.min")).unwrap(),
            "1.00\n"
        );
        let blk = root("blkio");
        populate(Controller::Blkio, &blk, None).unwrap();
        let w = |f, v: &str, off| write(f, &blk, &blk, v.as_bytes(), off, false);
        w(File::PrioClass, "idle\n", 0).unwrap();
        assert_eq!(
            fs::read_to_string(blk.join("blkio.prio.class")).unwrap(),
            "idle\n"
        );
        assert_eq!(w(File::PrioClass, "idle", 4), Err(EIO));
        assert_eq!(w(File::PrioClass, "fast", 0), Err(EINVAL));
        fs::remove_dir_all(&top).unwrap();
        fs::remove_dir_all(&blk).unwrap();
    }

    #[test]
    fn membership_round_trips_and_names_groups() {
        let m = Membership {
            processes: vec![(100, (1, 2), "/background".into())],
            threads: vec![(131172, (1, 2), "/top-app/x".into())],
        };
        assert_eq!(Membership::parse(&m.text()), m);
        assert!(m.names("/background"));
        assert!(m.names("/top-app"));
        assert!(!m.names("/top"));
        assert!(m.names("/"));
        assert_eq!(group_path(Path::new("/r/a/b"), Path::new("/r")), "/a/b");
        assert_eq!(
            state_path(Path::new("/run/cgroup-v1/cpu")),
            Path::new("/run/cgroup-v1/cpu.tasks")
        );
    }
}
