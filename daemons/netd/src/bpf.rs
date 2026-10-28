//! BpfHandler::initPrograms' socket-create attachment. The syscall layer
//! executes the program; netd only installs the loader's pinned object.
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

pub fn attach_socket_create() -> std::io::Result<()> {
    let mounts = std::fs::read_to_string("/proc/mounts")?;
    let mount = mounts
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            fields.next()?;
            let path = fields.next()?;
            (fields.next()? == "cgroup2").then_some(path)
        })
        .ok_or_else(|| std::io::Error::from_raw_os_error(libc::ENOENT))?;
    let cgroup = std::fs::File::open(mount)?;
    // union bpf_attr: pathname, bpf_fd, file_flags.
    let path = c"/sys/fs/bpf/netd_shared/prog_netd_cgroupsock_inet_create";
    let get = [path.as_ptr() as u64, 0u64];
    let fd = unsafe { libc::syscall(libc::SYS_bpf, 7, get.as_ptr(), std::mem::size_of_val(&get)) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let program = unsafe { OwnedFd::from_raw_fd(fd as i32) };
    // target_fd, attach_bpf_fd, BPF_CGROUP_INET_SOCK_CREATE, flags=0.
    let attach = [cgroup.as_raw_fd() as u32, program.as_raw_fd() as u32, 2, 0];
    let result = unsafe {
        libc::syscall(
            libc::SYS_bpf,
            8,
            attach.as_ptr(),
            std::mem::size_of_val(&attach),
        )
    };
    if result < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
