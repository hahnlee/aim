//! SELinux labels of filesystems without xattrs (`genfscon`): on Linux the
//! kernel labels a bpffs or cgroup2 inode by the longest `genfscon` path
//! prefix the loaded policy gives that filesystem. The policy is the
//! image's CIL (`/system/etc/selinux/plat_sepolicy.cil` and the partitions'
//! counterparts), which init compiles at boot.

use std::os::unix::ffi::OsStrExt;
use std::sync::OnceLock;

use crate::vfs;

const POLICIES: [&str; 5] = [
    "/system/etc/selinux/plat_sepolicy.cil",
    "/system_ext/etc/selinux/system_ext_sepolicy.cil",
    "/product/etc/selinux/product_sepolicy.cil",
    "/vendor/etc/selinux/vendor_sepolicy.cil",
    "/odm/etc/selinux/odm_sepolicy.cil",
];
const AT_FDCWD: i32 = -100;
/// CIL's optional file type of a `genfscon`.
const FILE_TYPES: [&str; 8] = [
    "file", "dir", "char", "block", "socket", "pipe", "symlink", "any",
];

/// (filesystem, path prefix, context).
type Rules = Vec<(String, String, String)>;

static RULES: OnceLock<Rules> = OnceLock::new();

fn rules() -> &'static Rules {
    RULES.get_or_init(|| {
        let mut rules = Vec::new();
        for policy in POLICIES {
            let Ok(r) = vfs::resolve(AT_FDCWD, policy.as_bytes(), true) else {
                continue;
            };
            let host = std::ffi::OsStr::from_bytes(r.host.to_bytes());
            let Ok(text) = std::fs::read(host) else {
                continue;
            };
            rules.extend(parse(&String::from_utf8_lossy(&text)));
        }
        rules
    })
}

/// `(genfscon FS "PATH" [FILETYPE] (USER ROLE TYPE ((LOW) (HIGH))))`.
fn parse(text: &str) -> Rules {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("(genfscon ") else {
            continue;
        };
        let words: Vec<&str> = rest
            .split(|c: char| c.is_whitespace() || c == '(' || c == ')')
            .filter(|w| !w.is_empty())
            .collect();
        // FS PATH [FILETYPE] USER ROLE TYPE LEVEL ...
        let typed = words.get(2).is_some_and(|w| FILE_TYPES.contains(w));
        let at = if typed { 3 } else { 2 };
        if words.len() < at + 4 {
            continue;
        }
        let [user, role, ty, level] = [words[at], words[at + 1], words[at + 2], words[at + 3]];
        out.push((
            words[0].to_string(),
            words[1].trim_matches('"').to_string(),
            format!("{user}:{role}:{ty}:{level}"),
        ));
    }
    out
}

fn lookup<'a>(rules: &'a Rules, fs: &str, path: &str) -> Option<&'a str> {
    rules
        .iter()
        .filter(|(f, prefix, _)| f == fs && path.starts_with(prefix.as_str()))
        .max_by_key(|(_, prefix, _)| prefix.len())
        .map(|(_, _, ctx)| ctx.as_str())
}

/// The context the kernel gives `guest` if it is on a genfs-labeled
/// filesystem.
pub fn label(guest: &str) -> Option<String> {
    let (fs, path) = vfs::fs_path(guest)?;
    lookup(rules(), &fs, &path).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CIL: &str = r#"
(genfscon bpf "/net_shared" (u object_r fs_bpf_net_shared ((s0) (s0))))
(genfscon bpf "/" (u object_r fs_bpf ((s0) (s0))))
(genfscon cgroup2 "/" (u object_r cgroup_v2 ((s0) (s0))))
(genfscon proc "/sys/kernel" dir (u object_r proc_kernel ((s0) (s0))))
(type fs_bpf)
"#;

    #[test]
    fn the_longest_prefix_labels() {
        let rules = parse(CIL);
        assert_eq!(rules.len(), 4);
        assert_eq!(lookup(&rules, "bpf", "/"), Some("u:object_r:fs_bpf:s0"));
        assert_eq!(
            lookup(&rules, "bpf", "/net_shared/map_x"),
            Some("u:object_r:fs_bpf_net_shared:s0")
        );
        assert_eq!(
            lookup(&rules, "bpf", "/netd_shared"),
            Some("u:object_r:fs_bpf:s0")
        );
        assert_eq!(
            lookup(&rules, "cgroup2", "/apps"),
            Some("u:object_r:cgroup_v2:s0")
        );
        assert_eq!(
            lookup(&rules, "proc", "/sys/kernel/x"),
            Some("u:object_r:proc_kernel:s0")
        );
        assert_eq!(lookup(&rules, "tmpfs", "/"), None);
    }
}
