//! Linux `sync_file` (include/uapi/linux/sync_file.h): the fences the GPU
//! and display drivers hand out (docs/graphics-buffers.md, "Fences").
//!
//! A sync_file is a host datagram socket made by `aim_sync_file`, which the
//! GPU module, the display server and [`SYNC_IOC_MERGE`] signal once. `poll`,
//! `epoll` and `select` wait for it as for any fd; here are its ioctls and
//! the rest of its Linux behaviour: `read` and `write` fail with `EINVAL`,
//! and `/proc/self/fd` shows it as `anon_inode:sync_file`.

use aim_sync_file::State;
use std::os::fd::{BorrowedFd,AsRawFd};

use super::fdtab::{self, Kind};
use super::fork_state::{Reader, Writer};
use crate::errno::{EFAULT, EINVAL, ENOENT, ENOTTY};

/// `_IOWR('>', 3, struct sync_merge_data)`.
const SYNC_IOC_MERGE: u64 = 0xc030_3e03;
/// `_IOWR('>', 4, struct sync_file_info)`.
const SYNC_IOC_FILE_INFO: u64 = 0xc038_3e04;
/// `_IOW('>', 5, struct sync_set_deadline)`.
const SYNC_IOC_SET_DEADLINE: u64 = 0x4010_3e05;
/// The ioctl type byte of `SYNC_IOC_*`.
const SYNC_IOC_MAGIC: u64 = b'>' as u64;

/// The name every fence reports (Linux: the driver and timeline).
const NAME: &[u8] = b"aim";
const DRIVER: &[u8] = b"aim";

#[repr(C)]
#[derive(Clone, Copy)]
struct MergeData {
    name: [u8; 32],
    fd2: i32,
    fence: i32,
    flags: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FileInfo {
    name: [u8; 32],
    status: i32,
    flags: u32,
    num_fences: u32,
    pad: u32,
    sync_fence_info: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FenceInfo {
    obj_name: [u8; 32],
    driver_name: [u8; 32],
    status: i32,
    flags: u32,
    timestamp_ns: u64,
}

const _: () = assert!(size_of::<MergeData>() == 48);
const _: () = assert!(size_of::<FileInfo>() == 56);
const _: () = assert!(size_of::<FenceInfo>() == 80);

fn name32(s: &[u8]) -> [u8; 32] {
    let mut n = [0; 32];
    n[..s.len()].copy_from_slice(s);
    n
}

/// Install the hooks through which `aim_sync_file` keeps its own fds out
/// of the guest's view and gives fences to the guest.
pub fn init() {
    aim_sync_file::set_hooks(aim_sync_file::Hooks {
        hide: fdtab::hide,
        unhide: fdtab::unhide,
        adopt:publish_fence,
    });
}

/// `fd` (new to the guest, or arrived from elsewhere) is a sync_file.
pub fn adopt(fd: i32) {
    fdtab::insert(fd, Kind::SyncFile);
}

fn publish_fence(fd:i32)->Result<(),crate::errno::Errno>{
    let _guard=fdtab::lifecycle();
    adopt(fd);
    match fdtab::publish_typed_guest(fd){Ok(())=>Ok(()),Err(error)=>{fdtab::on_close(fd);Err(error)}}
}

/// Whether `fd` is a sync_file; one that arrived unrecognized is adopted.
fn is_sync_file(fd: i32) -> bool {
    match fdtab::get(fd) {
        Some(Kind::SyncFile) => true,
        Some(_) => false,
        // SAFETY: borrowed only for the check; a closed fd is not a socket.
        None if aim_sync_file::is_sync_file(unsafe { BorrowedFd::borrow_raw(fd) }) => {
            adopt(fd);
            true
        }
        None => false,
    }
}

fn state(fd: i32) -> State {
    // SAFETY: a guest fd checked to be a sync_file, borrowed for the call.
    aim_sync_file::state(unsafe { BorrowedFd::borrow_raw(fd) })
}

/// The `SYNC_IOC_*` ioctls. None: not a sync_file request.
pub fn ioctl(fd: i32, req: u64, arg: u64) -> Option<i64> {
    if (req >> 8) & 0xff != SYNC_IOC_MAGIC || !is_sync_file(fd) {
        return None;
    }
    if arg == 0 {
        return Some(-(EFAULT as i64));
    }
    Some(match req {
        SYNC_IOC_MERGE => merge(fd, arg),
        SYNC_IOC_FILE_INFO => file_info(fd, arg),
        SYNC_IOC_SET_DEADLINE => {
            let bytes=match super::user_memory::read_exact(arg,16){Ok(bytes)=>bytes,Err(error)=>return Some(-(error as i64))};
            let d=unsafe{(bytes.as_ptr()as *const [u64;2]).read_unaligned()};
            // The deadline is a hint; the host GPU has no clock to boost.
            if d[1] != 0 { -(EINVAL as i64) } else { 0 }
        }
        _ => -(ENOTTY as i64),
    })
}

fn merge(fd: i32, arg: u64) -> i64 {
    let bytes=match super::user_memory::read_exact(arg,size_of::<MergeData>()){Ok(bytes)=>bytes,Err(error)=>return -(error as i64)};
    let mut d=unsafe{(bytes.as_ptr()as *const MergeData).read_unaligned()};
    if d.flags != 0 || d.pad != 0 {return -(EINVAL as i64);}
    let second=match fdtab::pin_guest(d.fd2){Ok(pin)=>pin,Err(crate::errno::EBADF)=>return -(ENOENT as i64),Err(error)=>return -(error as i64)};
    if !is_sync_file(second.descriptor().as_raw_fd()){return -(ENOENT as i64);}
    let file=match unsafe{aim_sync_file::merge(BorrowedFd::borrow_raw(fd),second.descriptor())}{Ok(file)=>file,Err(error)=>return -(crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::ENOMEM))as i64)};
    // The new descriptor remains owned and unpublished until copy_to_user
    // succeeds, as sync_file_ioctl_merge's fd_install ordering requires.
    d.name[31]=0;
    d.fence=file.as_raw_fd();
    let output=unsafe{std::slice::from_raw_parts((&d as *const MergeData).cast::<u8>(),size_of::<MergeData>())};
    if let Err(error)=super::user_memory::write_exact(arg,output){return -(error as i64);}
    match aim_sync_file::give_to_guest(file){Ok(_)=>0,Err(error)=>-(error as i64)}
}

fn file_info(fd: i32, arg: u64) -> i64 {
    let bytes=match super::user_memory::read_exact(arg,size_of::<FileInfo>()){Ok(bytes)=>bytes,Err(error)=>return -(error as i64)};
    let mut info=unsafe{(bytes.as_ptr()as *const FileInfo).read_unaligned()};
    if info.flags != 0 || info.pad != 0 {
        return -(EINVAL as i64);
    }
    // One fence per file: a merge signals its own fence once all of its
    // inputs have, with the last time and the first error.
    let (status, timestamp_ns) = match state(fd) {
        State::Active => (0, 0),
        State::Signaled {
            timestamp_ns,
            status,
        } => (status, timestamp_ns as u64),
    };
    if info.num_fences != 0 {
        if info.sync_fence_info == 0 {
            return -(EFAULT as i64);
        }
        let fence = FenceInfo {
            obj_name: name32(NAME),
            driver_name: name32(DRIVER),
            status,
            flags: 0,
            timestamp_ns,
        };
        let output=unsafe{std::slice::from_raw_parts((&fence as *const FenceInfo).cast::<u8>(),size_of::<FenceInfo>())};
        if let Err(error)=super::user_memory::write_exact(info.sync_fence_info,output){return -(error as i64);}
    }
    info.status = status;
    info.name = name32(NAME);
    info.num_fences = 1;
    let output=unsafe{std::slice::from_raw_parts((&info as *const FileInfo).cast::<u8>(),size_of::<FileInfo>())};
    super::user_memory::write_exact(arg,output).map(|_|0).unwrap_or_else(|error|-(error as i64))
}

/// The fds this process keeps for its pending fences, which a fork child
/// inherits.
pub(super) fn fork_save(w: &mut Writer) {
    w.seq(aim_sync_file::inherited().into_iter(), |w, fd| {w.retain_private(fd);w.i32(fd)});
}

/// A fork child closes the parent's pending-fence fds: the parent signals
/// those fences.
pub(super) fn fork_restore(r: &mut Reader) {
    init();
    for fd in r.seq(|r|r.i32()){if let Err(error)=fdtab::close_fork_private(fd){crate::diag!("fork sync private close: errno {error}");r.invalidate();return;}}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new guest sync_file and its writer.
    fn guest_fence() -> (i32, aim_sync_file::Writer) {
        let (file, writer) = aim_sync_file::pair().unwrap();
        let fd=aim_sync_file::give_to_guest(file).unwrap();
        (fd,writer)
    }

    /// Close a guest fd the way the guest does.
    fn close(fd: i32) {
        assert_eq!(super::super::fs::close([fd as u64,0,0,0,0,0]),0);
    }

    fn info(fd: i32, with_fence: bool) -> (i64, FileInfo, FenceInfo) {
        // SAFETY: plain old data.
        let mut fence: FenceInfo = unsafe { std::mem::zeroed() };
        // SAFETY: plain old data.
        let mut info: FileInfo = unsafe { std::mem::zeroed() };
        if with_fence {
            info.num_fences = 1;
            info.sync_fence_info = &mut fence as *mut FenceInfo as u64;
        }
        let r = ioctl(fd, SYNC_IOC_FILE_INFO, &mut info as *mut FileInfo as u64).unwrap();
        (r, info, fence)
    }

    #[test]
    fn file_info_reports_the_state_and_signal_time() {
        init();
        let (fd, writer) = guest_fence();
        let (r, i, _) = info(fd, false);
        assert_eq!((r, i.status, i.num_fences), (0, 0, 1));
        assert_eq!(&i.name[..4], b"aim\0");
        writer.signal_at(1234, 1);
        let (r, i, f) = info(fd, true);
        assert_eq!((r, i.status, i.num_fences), (0, 1, 1));
        assert_eq!((f.status, f.timestamp_ns), (1, 1234));
        assert_eq!(&f.driver_name[..4], b"aim\0");
        // Flags must be zero.
        // SAFETY: plain old data.
        let mut bad: FileInfo = unsafe { std::mem::zeroed() };
        bad.flags = 1;
        let r = ioctl(fd, SYNC_IOC_FILE_INFO, &mut bad as *mut FileInfo as u64);
        assert_eq!(r, Some(-(EINVAL as i64)));
        close(fd);
    }

    #[test]
    fn merge_makes_a_new_fence_and_checks_its_arguments() {
        init();
        let (a, wa) = guest_fence();
        let (b, wb) = guest_fence();
        // SAFETY: plain old data.
        let mut d: MergeData = unsafe { std::mem::zeroed() };
        d.fd2 = b;
        let r = ioctl(a, SYNC_IOC_MERGE, &mut d as *mut MergeData as u64);
        assert_eq!(r, Some(0));
        let m = d.fence;
        assert!(m > 2 && m != a && m != b);
        assert!(matches!(fdtab::get(m), Some(Kind::SyncFile)));
        assert_eq!(info(m, false).1.status, 0);
        wa.signal_at(5, 1);
        wb.signal_at(7, 1);
        // SAFETY: a guest fd borrowed for the wait.
        assert!(aim_sync_file::wait(
            unsafe { BorrowedFd::borrow_raw(m) },
            5000
        ));
        let (_, i, f) = info(m, true);
        assert_eq!((i.status, f.timestamp_ns), (1, 7));

        // fd2 that is not a sync_file, and nonzero flags.
        let (s, t) = std::os::unix::net::UnixDatagram::pair().unwrap();
        let other = std::os::fd::AsRawFd::as_raw_fd(&s);
        fdtab::publish_guest(other).unwrap();
        d.fd2 = other;
        let r = ioctl(a, SYNC_IOC_MERGE, &mut d as *mut MergeData as u64);
        assert_eq!(r, Some(-(ENOENT as i64)));
        d.fd2 = b;
        d.flags = 1;
        let r = ioctl(a, SYNC_IOC_MERGE, &mut d as *mut MergeData as u64);
        assert_eq!(r, Some(-(EINVAL as i64)));
        // Other sockets are not sync_files, and their ioctls go elsewhere.
        let r = ioctl(other, SYNC_IOC_FILE_INFO, &mut d as *mut MergeData as u64);
        assert_eq!(r, None);
        fdtab::withdraw_guest(other).unwrap();drop((s, t));
        for fd in [a, b, m] {
            close(fd);
        }
    }

    #[test]
    fn real_guest_merge_publishes_fd_and_fault_copy_rolls_back(){
        const MARKER:&str="sync-merge-native-namespace";
        if !std::env::args().any(|arg|arg==MARKER){
            let mut child=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","sys::sync_file::tests::real_guest_merge_publishes_fd_and_fault_copy_rolls_back","--skip",MARKER,"--nocapture"]).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);loop{if child.try_wait().unwrap().is_some(){break;}if std::time::Instant::now()>=deadline{child.kill().unwrap();child.wait().unwrap();panic!("merge namespace timed out");}std::thread::sleep(std::time::Duration::from_millis(5));}
            let output=child.wait_with_output().unwrap();assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));assert!(String::from_utf8_lossy(&output.stdout).contains("SYNC_MERGE_NATIVE_NAMESPACE_EXECUTED"));return;
        }
        init();let(a,wa)=guest_fence();let(b,wb)=guest_fence();wa.signal(1);wb.signal(1);
        let mut data:MergeData=unsafe{std::mem::zeroed()};data.fd2=b;
        let invoke=|at:u64|super::super::fs::ioctl([a as u64,SYNC_IOC_MERGE,at,0,0,0]);
        assert_eq!(invoke(&mut data as *mut MergeData as u64),0);let merged=data.fence;
        assert_eq!(super::super::fs::fcntl([merged as u64,1,0,0,0,0]),1);
        let mut stat=[0u8;128];assert_eq!(super::super::fs::fstat([merged as u64,stat.as_mut_ptr()as u64,0,0,0,0]),0);
        let mut poll=libc::pollfd{fd:merged,events:libc::POLLIN,revents:0};let timeout=[0i64;2];assert_eq!(super::super::poll::ppoll([&mut poll as *mut libc::pollfd as u64,1,timeout.as_ptr()as u64,0,0,0]),1);assert_ne!(poll.revents&libc::POLLIN,0);
        assert_eq!(super::super::fs::ioctl([merged as u64,SYNC_IOC_FILE_INFO,&mut (unsafe{std::mem::zeroed::<FileInfo>()})as *mut FileInfo as u64,0,0,0]),0);
        assert_eq!(super::super::fs::ioctl([merged as u64,SYNC_IOC_FILE_INFO,1,0,0,0]),-(EFAULT as i64));
        assert_eq!(super::super::fs::ioctl([merged as u64,SYNC_IOC_SET_DEADLINE,1,0,0,0]),-(EFAULT as i64));
        close(merged);
        data.fd2=-1;assert_eq!(invoke(&mut data as *mut MergeData as u64),-(ENOENT as i64));
        use std::os::fd::AsRawFd;
        let private=aim_storage::private_fd::PrivateFd::allocate(||std::fs::File::open("/dev/null").map(Into::into)).unwrap();data.fd2=private.as_raw_fd();assert_eq!(invoke(&mut data as *mut MergeData as u64),-(ENOENT as i64));drop(private);
        fn native_fds()->Vec<i32>{let mut bytes=[0u8;8192];let size=unsafe{libc::proc_pidinfo(libc::getpid(),1,0,bytes.as_mut_ptr().cast(),bytes.len()as i32)};assert!(size>=0);let mut fds=bytes[..size as usize].chunks_exact(8).map(|bytes|i32::from_ne_bytes(bytes[..4].try_into().unwrap())).collect::<Vec<_>>();fds.sort_unstable();fds}
        let page=super::super::mem::PAGE as usize;let memory=unsafe{libc::mmap(std::ptr::null_mut(),page,libc::PROT_READ|libc::PROT_WRITE,libc::MAP_ANON|libc::MAP_PRIVATE,-1,0)};assert_ne!(memory,libc::MAP_FAILED);data.fd2=b;unsafe{(memory as *mut MergeData).write(data);}assert_eq!(unsafe{libc::mprotect(memory,page,libc::PROT_READ)},0);
        let before=native_fds();assert_eq!(invoke(memory as u64),-(crate::errno::EFAULT as i64));assert_eq!(native_fds(),before,"unpublished merged FD must close on copy_to_user failure");assert_eq!(unsafe{libc::munmap(memory,page)},0);
        let rejected=aim_sync_file::signaled(1,1).unwrap();let fd=rejected.as_raw_fd();fdtab::keep_hidden(fd);
        assert_eq!(aim_sync_file::give_to_guest(rejected),Err(crate::errno::EBADF));
        assert_eq!(unsafe{libc::fcntl(fd,libc::F_GETFD)},-1);assert_eq!(crate::errno::last(),crate::errno::EBADF);assert!(fdtab::get(fd).is_none());fdtab::unhide(fd);
        close(a);close(b);println!("SYNC_MERGE_NATIVE_NAMESPACE_EXECUTED");
    }

    #[test]
    fn a_fence_from_elsewhere_is_recognized() {
        const MARKER:&str="foreign-fence-native-namespace";
        if !std::env::args().any(|arg|arg==MARKER){
            let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","sys::sync_file::tests::a_fence_from_elsewhere_is_recognized","--skip",MARKER,"--nocapture"]).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
            let mut child=output;let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);
            loop{if child.try_wait().unwrap().is_some(){break;}if std::time::Instant::now()>=deadline{child.kill().unwrap();child.wait().unwrap();panic!("native fixture namespace timed out");}std::thread::sleep(std::time::Duration::from_millis(5));}
            let output=child.wait_with_output().unwrap();
            assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));assert!(String::from_utf8_lossy(&output.stdout).contains("FOREIGN_FENCE_NATIVE_NAMESPACE_EXECUTED"));return;
        }

        init();
        let (file, writer) = aim_sync_file::pair().unwrap();
        // Arrived by SCM_RIGHTS or binder: not in the table yet.
        let fd = std::os::fd::IntoRawFd::into_raw_fd(file);
        fdtab::publish_guest(fd).unwrap();
        assert!(fdtab::get(fd).is_none());
        writer.signal(1);
        assert_eq!(info(fd, false).1.status, 1);
        assert!(matches!(fdtab::get(fd), Some(Kind::SyncFile)));
        let r = ioctl(fd, SYNC_IOC_SET_DEADLINE, [0u64; 2].as_ptr() as u64);
        assert_eq!(r, Some(0));
        let r = ioctl(fd, 0xc0083e7f, [0u64; 2].as_ptr() as u64);
        assert_eq!(r, Some(-(ENOTTY as i64)));
        close(fd);
        println!("FOREIGN_FENCE_NATIVE_NAMESPACE_EXECUTED");
    }
}
