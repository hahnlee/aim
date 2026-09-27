//! `android.net.LocalSocketImpl` natives (android_net_LocalSocketImpl.cpp).
//!
//! libcutils names local sockets in three namespaces. FILESYSTEM names are
//! guest paths and RESERVED names live under `/dev/socket/`; both resolve
//! through the bionic filesystem facade to their host backing, and are bound
//! or connected relative to the parent directory (the calling thread's
//! working directory, `pthread_fchdir_np`) so a long profile path does not hit
//! Darwin's 104-byte `sun_path`. Darwin has no
//! abstract namespace; ABSTRACT names fail with EOPNOTSUPP until a profile-wide
//! registry provides one.
//!
//! Descriptors travel as SCM_RIGHTS ancillary data exactly as on Android, and
//! peer credentials report the peer's Android identity from the profile's
//! process registry rather than its host uid.

use crate::jni_env::{Env, native};
use jni_sys::{JNIEnv, jbyteArray, jclass, jfieldID, jint, jmethodID, jobject, jstring, jvalue};
use std::ffi::{CString, OsStr, c_void};
use std::os::raw::{c_char, c_int};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// LocalSocketAddress.Namespace ids (libcutils socket_local.h).
const NAMESPACE_ABSTRACT: jint = 0;
const NAMESPACE_RESERVED: jint = 1;
const NAMESPACE_FILESYSTEM: jint = 2;
const RESERVED_PREFIX: &str = "/dev/socket/";
/// ReceiveFileDescriptorVector's bound in socket_read_all.
const MAX_RECEIVED_FDS: usize = 64;

/// The guest path a namespaced local socket name refers to.
fn guest_path(name: &str, namespace: jint) -> Result<String, c_int> {
    match namespace {
        NAMESPACE_RESERVED => Ok(format!("{RESERVED_PREFIX}{name}")),
        NAMESPACE_FILESYSTEM => Ok(name.to_owned()),
        NAMESPACE_ABSTRACT => Err(libc::EOPNOTSUPP),
        _ => Err(libc::EINVAL),
    }
}

type ResolveHostPath = unsafe extern "C" fn(*const c_char, *mut c_char, usize) -> isize;

/// The facade's guest-to-host mapping for writable mounts, when this process
/// runs behind the facade.
fn resolver() -> Option<ResolveHostPath> {
    static RESOLVER: OnceLock<usize> = OnceLock::new();
    let address = *RESOLVER.get_or_init(|| unsafe {
        libc::dlsym(
            libc::RTLD_DEFAULT,
            c"darwin_art_bionic_fs_resolve_writable_host_path".as_ptr(),
        ) as usize
    });
    // SAFETY: the symbol has the facade's exported signature.
    (address != 0).then(|| unsafe { std::mem::transmute::<usize, ResolveHostPath>(address) })
}

fn host_path(guest: &str) -> Result<PathBuf, c_int> {
    let resolve = resolver().ok_or(libc::EOPNOTSUPP)?;
    let guest = CString::new(guest).map_err(|_| libc::EINVAL)?;
    let mut buffer = vec![0u8; libc::PATH_MAX as usize];
    let length = unsafe { resolve(guest.as_ptr(), buffer.as_mut_ptr().cast(), buffer.len()) };
    if length < 0 {
        return Err(std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::ENOENT));
    }
    buffer.truncate(length as usize);
    Ok(PathBuf::from(OsStr::from_bytes(&buffer)))
}

unsafe extern "C" {
    /// libsystem_pthread: set the calling thread's working directory; -1
    /// returns the thread to the process working directory.
    fn pthread_fchdir_np(fd: c_int) -> c_int;
}

/// A socket address naming `leaf` inside an open parent directory.
struct RelativeAddress {
    directory: c_int,
    leaf: CString,
    address: libc::sockaddr_un,
    length: libc::socklen_t,
}

impl RelativeAddress {
    fn open(path: &Path) -> Result<Self, c_int> {
        let parent = path.parent().ok_or(libc::EINVAL)?;
        let leaf = path.file_name().ok_or(libc::EINVAL)?.as_bytes();
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        if leaf.len() >= address.sun_path.len() {
            return Err(libc::ENAMETOOLONG);
        }
        for (slot, byte) in address.sun_path.iter_mut().zip(leaf) {
            *slot = *byte as c_char;
        }
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        let length =
            (std::mem::offset_of!(libc::sockaddr_un, sun_path) + leaf.len() + 1) as libc::socklen_t;
        address.sun_len = length as u8;
        let parent = CString::new(parent.as_os_str().as_bytes()).map_err(|_| libc::EINVAL)?;
        let directory = unsafe {
            libc::open(
                parent.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if directory < 0 {
            return Err(errno());
        }
        Ok(Self {
            directory,
            leaf: CString::new(leaf).map_err(|_| libc::EINVAL)?,
            address,
            length,
        })
    }

    fn sockaddr(&self) -> *const libc::sockaddr {
        (&self.address as *const libc::sockaddr_un).cast()
    }

    /// Run a socket call on the leaf with this thread inside the directory.
    fn within<T>(
        &self,
        call: impl FnOnce(*const libc::sockaddr, libc::socklen_t) -> T,
    ) -> Result<T, c_int> {
        if unsafe { pthread_fchdir_np(self.directory) } != 0 {
            return Err(errno());
        }
        let result = call(self.sockaddr(), self.length);
        let saved = errno();
        unsafe { pthread_fchdir_np(-1) };
        // Keep the call's errno for the caller's error path.
        unsafe { *libc::__error() = saved };
        Ok(result)
    }

    fn bind(&self, fd: c_int) -> Result<(), c_int> {
        match self.within(|address, length| unsafe { libc::bind(fd, address, length) })? {
            0 => Ok(()),
            _ => Err(errno()),
        }
    }

    fn connect(&self, fd: c_int) -> Result<(), c_int> {
        loop {
            match self.within(|address, length| unsafe { libc::connect(fd, address, length) })? {
                0 => return Ok(()),
                _ if errno() == libc::EINTR => continue,
                _ => return Err(errno()),
            }
        }
    }
}

impl Drop for RelativeAddress {
    fn drop(&mut self) {
        unsafe { libc::close(self.directory) };
    }
}

fn errno() -> c_int {
    std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EIO)
}

/// socket_local_client_connect for a SOCK_STREAM LocalSocket.
fn connect(fd: c_int, name: &str, namespace: jint) -> Result<(), c_int> {
    RelativeAddress::open(&host_path(&guest_path(name, namespace)?)?)?.connect(fd)
}

/// socket_local_server_bind: a stale filesystem entry is replaced.
fn bind(fd: c_int, name: &str, namespace: jint) -> Result<(), c_int> {
    let address = RelativeAddress::open(&host_path(&guest_path(name, namespace)?)?)?;
    unsafe { libc::unlinkat(address.directory, address.leaf.as_ptr(), 0) };
    address.bind(fd)
}

/// ReceiveFileDescriptorVector: data plus any SCM_RIGHTS descriptors
/// (close-on-exec, as MSG_CMSG_CLOEXEC gives on Linux).
fn receive(fd: c_int, buffer: &mut [u8]) -> Result<(usize, Vec<c_int>), c_int> {
    let space =
        unsafe { libc::CMSG_SPACE((MAX_RECEIVED_FDS * size_of::<c_int>()) as u32) } as usize;
    let mut control = vec![0u8; space];
    let mut iov = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = space as libc::socklen_t;
    let received = loop {
        let count = unsafe { libc::recvmsg(fd, &mut message, 0) };
        if count >= 0 {
            break count as usize;
        }
        let error = errno();
        if error != libc::EINTR {
            return Err(error);
        }
    };
    let mut descriptors = Vec::new();
    let mut header = unsafe { libc::CMSG_FIRSTHDR(&message) };
    while !header.is_null() {
        let entry = unsafe { &*header };
        if entry.cmsg_level == libc::SOL_SOCKET && entry.cmsg_type == libc::SCM_RIGHTS {
            let data = unsafe { libc::CMSG_DATA(header) };
            let bytes = entry.cmsg_len as usize - (data as usize - header as usize);
            for index in 0..bytes / size_of::<c_int>() {
                let descriptor =
                    unsafe { std::ptr::read_unaligned(data.cast::<c_int>().add(index)) };
                unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) };
                descriptors.push(descriptor);
            }
        }
        header = unsafe { libc::CMSG_NXTHDR(&message, header) };
    }
    if message.msg_flags & libc::MSG_CTRUNC != 0 {
        for descriptor in descriptors {
            unsafe { libc::close(descriptor) };
        }
        return Err(libc::EMSGSIZE);
    }
    Ok((received, descriptors))
}

/// SendFileDescriptorVector followed by plain sends of the remainder.
fn send_all(fd: c_int, mut data: &[u8], descriptors: &[c_int]) -> Result<(), c_int> {
    // Darwin has no MSG_NOSIGNAL; a closed peer reports EPIPE instead.
    let one: c_int = 1;
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            (&one as *const c_int).cast(),
            size_of::<c_int>() as libc::socklen_t,
        )
    };
    let mut first = true;
    while first || !data.is_empty() {
        let mut iov = libc::iovec {
            iov_base: data.as_ptr() as *mut c_void,
            iov_len: data.len(),
        };
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        let mut control = Vec::new();
        if first && !descriptors.is_empty() {
            let payload = size_of_val(descriptors) as u32;
            control = vec![0u8; unsafe { libc::CMSG_SPACE(payload) } as usize];
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen = control.len() as libc::socklen_t;
            let header = unsafe { libc::CMSG_FIRSTHDR(&message) };
            unsafe {
                (*header).cmsg_level = libc::SOL_SOCKET;
                (*header).cmsg_type = libc::SCM_RIGHTS;
                (*header).cmsg_len = libc::CMSG_LEN(payload) as libc::socklen_t;
                std::ptr::copy_nonoverlapping(
                    descriptors.as_ptr(),
                    libc::CMSG_DATA(header).cast::<c_int>(),
                    descriptors.len(),
                );
            }
        }
        let sent = unsafe { libc::sendmsg(fd, &message, 0) };
        drop(control);
        if sent < 0 {
            let error = errno();
            if error == libc::EINTR {
                continue;
            }
            return Err(error);
        }
        first = false;
        data = &data[sent as usize..];
    }
    Ok(())
}

struct Ids {
    descriptor: jfieldID,
    file_descriptor: jclass,
    file_descriptor_init: jmethodID,
    inbound: jfieldID,
    outbound: jfieldID,
    credentials: jclass,
    credentials_init: jmethodID,
}

// JNI ids and global class references are process-wide.
unsafe impl Send for Ids {}
unsafe impl Sync for Ids {}

static IDS: OnceLock<Ids> = OnceLock::new();

fn ids() -> &'static Ids {
    IDS.get()
        .expect("LocalSocketImpl natives used before registration")
}

/// libnativehelper jniGetFDFromFileDescriptor.
fn fd_of(env: Env, descriptor: jobject) -> Option<c_int> {
    if descriptor.is_null() {
        env.throw_null_pointer();
        return None;
    }
    Some(env.int_field(descriptor, ids().descriptor))
}

/// libnativehelper jniCreateFileDescriptor.
fn new_file_descriptor(env: Env, fd: c_int) -> jobject {
    let ids = ids();
    let object = env.new_object(ids.file_descriptor, ids.file_descriptor_init, &[]);
    if !object.is_null() {
        env.set_int_field(object, ids.descriptor, fd);
    }
    object
}

/// socket_read_all: the byte count (0 at end of stream), or None with an
/// exception pending. Received descriptors are published to the socket.
fn read_all(env: Env, socket: jobject, fd: c_int, buffer: &mut [u8]) -> Option<usize> {
    let (count, descriptors) = match receive(fd, buffer) {
        Ok(result) => result,
        Err(libc::EPIPE) => return Some(0),
        Err(error) => {
            env.throw_io(error);
            return None;
        }
    };
    if !descriptors.is_empty() {
        let array = env.new_object_array(descriptors.len() as jint, ids().file_descriptor);
        if array.is_null() {
            descriptors.iter().for_each(|fd| unsafe {
                libc::close(*fd);
            });
            return None;
        }
        for (index, fd) in descriptors.iter().enumerate() {
            let object = new_file_descriptor(env, *fd);
            if env.exception_check() {
                return None;
            }
            env.set_object_array_element(array, index as jint, object);
            env.delete_local_ref(object);
        }
        env.set_object_field(socket, ids().inbound, array);
    }
    Some(count)
}

/// socket_write_all, with the socket's pending outbound descriptors.
fn write_all(env: Env, socket: jobject, fd: c_int, data: &[u8]) {
    let outbound = env.object_field(socket, ids().outbound);
    if env.exception_check() {
        return;
    }
    let mut descriptors = Vec::new();
    if !outbound.is_null() {
        for index in 0..env.array_length(outbound) {
            let object = env.object_array_element(outbound, index);
            let Some(descriptor) = fd_of(env, object) else {
                return;
            };
            descriptors.push(descriptor);
            env.delete_local_ref(object);
        }
    }
    if let Err(error) = send_all(fd, data, &descriptors) {
        env.throw_io(error);
    }
}

fn array_range(env: Env, buffer: jbyteArray, offset: jint, length: jint) -> bool {
    if offset < 0 || length < 0 || offset as i64 + length as i64 > env.array_length(buffer) as i64 {
        env.throw(c"java/lang/ArrayIndexOutOfBoundsException", "");
        return false;
    }
    true
}

unsafe extern "system" fn connect_local(
    env: *mut JNIEnv,
    _socket: jobject,
    descriptor: jobject,
    name: jstring,
    namespace: jint,
) {
    let env = unsafe { Env::new(env) };
    let Some(name) = env.string(name) else {
        env.throw_null_pointer();
        return;
    };
    let Some(fd) = fd_of(env, descriptor) else {
        return;
    };
    if let Err(error) = connect(fd, &name, namespace) {
        env.throw_io(error);
    }
}

unsafe extern "system" fn bind_local(
    env: *mut JNIEnv,
    _socket: jobject,
    descriptor: jobject,
    name: jstring,
    namespace: jint,
) {
    let env = unsafe { Env::new(env) };
    let Some(name) = env.string(name) else {
        env.throw_null_pointer();
        return;
    };
    let Some(fd) = fd_of(env, descriptor) else {
        return;
    };
    if let Err(error) = bind(fd, &name, namespace) {
        env.throw_io(error);
    }
}

unsafe extern "system" fn read_native(
    env: *mut JNIEnv,
    socket: jobject,
    descriptor: jobject,
) -> jint {
    let env = unsafe { Env::new(env) };
    let Some(fd) = fd_of(env, descriptor) else {
        return -1;
    };
    let mut byte = [0u8; 1];
    match read_all(env, socket, fd, &mut byte) {
        None => 0,
        Some(0) => -1,
        Some(_) => byte[0] as jint,
    }
}

unsafe extern "system" fn readba_native(
    env: *mut JNIEnv,
    socket: jobject,
    buffer: jbyteArray,
    offset: jint,
    length: jint,
    descriptor: jobject,
) -> jint {
    let env = unsafe { Env::new(env) };
    if descriptor.is_null() || buffer.is_null() {
        env.throw_null_pointer();
        return -1;
    }
    if !array_range(env, buffer, offset, length) {
        return -1;
    }
    if length == 0 {
        return 0;
    }
    let Some(fd) = fd_of(env, descriptor) else {
        return -1;
    };
    let mut data = vec![0u8; length as usize];
    match read_all(env, socket, fd, &mut data) {
        None | Some(0) => -1,
        Some(count) => {
            env.set_byte_array_region(buffer, offset, &data[..count]);
            count as jint
        }
    }
}

unsafe extern "system" fn write_native(
    env: *mut JNIEnv,
    socket: jobject,
    value: jint,
    descriptor: jobject,
) {
    let env = unsafe { Env::new(env) };
    let Some(fd) = fd_of(env, descriptor) else {
        return;
    };
    // socket_write sends the low byte of the int (little-endian &b).
    write_all(env, socket, fd, &[value as u8]);
}

unsafe extern "system" fn writeba_native(
    env: *mut JNIEnv,
    socket: jobject,
    buffer: jbyteArray,
    offset: jint,
    length: jint,
    descriptor: jobject,
) {
    let env = unsafe { Env::new(env) };
    if descriptor.is_null() || buffer.is_null() {
        env.throw_null_pointer();
        return;
    }
    if !array_range(env, buffer, offset, length) {
        return;
    }
    let Some(fd) = fd_of(env, descriptor) else {
        return;
    };
    let mut data = vec![0u8; length as usize];
    env.byte_array_region(buffer, offset, &mut data);
    write_all(env, socket, fd, &data);
}

/// The Android uid of a peer process: this process's own, or the profile
/// registry's record of it.
fn android_uid(pid: libc::pid_t) -> Option<u32> {
    if pid == unsafe { libc::getpid() }
        && let Some(uid) = std::env::var("DARWIN_ART_ANDROID_UID")
            .ok()
            .and_then(|value| value.parse().ok())
    {
        return Some(uid);
    }
    let socket = std::env::var_os(darwin_art_profile::PROFILE_SOCKET_ENV)?;
    darwin_art_profile::resolve_process_identity_at(Path::new(&socket), pid as u32)
        .ok()
        .map(|identity| identity.uid)
}

unsafe extern "system" fn get_peer_credentials(
    env: *mut JNIEnv,
    _socket: jobject,
    descriptor: jobject,
) -> jobject {
    let env = unsafe { Env::new(env) };
    let Some(fd) = fd_of(env, descriptor) else {
        return std::ptr::null_mut();
    };
    let mut pid: libc::pid_t = 0;
    let mut length = size_of::<libc::pid_t>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut length,
        )
    } != 0
    {
        env.throw_io(errno());
        return std::ptr::null_mut();
    }
    // Only a registered Android process has an Android identity.
    let Some(uid) = android_uid(pid) else {
        env.throw_io(libc::EPERM);
        return std::ptr::null_mut();
    };
    let ids = ids();
    // Android app and system processes run with gid == uid.
    let arguments = [
        jvalue { i: pid },
        jvalue { i: uid as jint },
        jvalue { i: uid as jint },
    ];
    env.new_object(ids.credentials, ids.credentials_init, &arguments)
}

/// register_android_net_LocalSocketImpl.
///
/// # Safety
/// `env` must be the calling thread's JNIEnv.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_register_local_socket_natives(env: *mut JNIEnv) -> bool {
    let env = unsafe { Env::new(env) };
    let socket = env.find_class(c"android/net/LocalSocketImpl");
    let file_descriptor = env.find_class(c"java/io/FileDescriptor");
    let credentials = env.find_class(c"android/net/Credentials");
    if socket.is_null() || file_descriptor.is_null() || credentials.is_null() {
        return false;
    }
    let ids = Ids {
        descriptor: env.field_id(file_descriptor, c"descriptor", c"I"),
        file_descriptor_init: env.method_id(file_descriptor, c"<init>", c"()V"),
        file_descriptor: env.new_global_ref(file_descriptor),
        inbound: env.field_id(
            socket,
            c"inboundFileDescriptors",
            c"[Ljava/io/FileDescriptor;",
        ),
        outbound: env.field_id(
            socket,
            c"outboundFileDescriptors",
            c"[Ljava/io/FileDescriptor;",
        ),
        credentials_init: env.method_id(credentials, c"<init>", c"(III)V"),
        credentials: env.new_global_ref(credentials),
    };
    if env.exception_check()
        || ids.descriptor.is_null()
        || ids.file_descriptor_init.is_null()
        || ids.inbound.is_null()
        || ids.outbound.is_null()
        || ids.credentials_init.is_null()
    {
        return false;
    }
    let _ = IDS.set(ids);
    let methods = [
        native(
            c"connectLocal",
            c"(Ljava/io/FileDescriptor;Ljava/lang/String;I)V",
            connect_local as *mut c_void,
        ),
        native(
            c"bindLocal",
            c"(Ljava/io/FileDescriptor;Ljava/lang/String;I)V",
            bind_local as *mut c_void,
        ),
        native(
            c"read_native",
            c"(Ljava/io/FileDescriptor;)I",
            read_native as *mut c_void,
        ),
        native(
            c"readba_native",
            c"([BIILjava/io/FileDescriptor;)I",
            readba_native as *mut c_void,
        ),
        native(
            c"writeba_native",
            c"([BIILjava/io/FileDescriptor;)V",
            writeba_native as *mut c_void,
        ),
        native(
            c"write_native",
            c"(ILjava/io/FileDescriptor;)V",
            write_native as *mut c_void,
        ),
        native(
            c"getPeerCredentials_native",
            c"(Ljava/io/FileDescriptor;)Landroid/net/Credentials;",
            get_peer_credentials as *mut c_void,
        ),
    ];
    let registered = env.register_natives(socket, &methods);
    env.delete_local_ref(socket);
    env.delete_local_ref(file_descriptor);
    env.delete_local_ref(credentials);
    registered
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixDatagram;

    #[test]
    fn namespaces_name_guest_paths() {
        assert_eq!(
            guest_path("zygote", NAMESPACE_RESERVED).unwrap(),
            "/dev/socket/zygote"
        );
        assert_eq!(
            guest_path("/data/system/unsolzygotesocket", NAMESPACE_FILESYSTEM).unwrap(),
            "/data/system/unsolzygotesocket"
        );
        assert_eq!(
            guest_path("jdwp", NAMESPACE_ABSTRACT),
            Err(libc::EOPNOTSUPP)
        );
        assert_eq!(guest_path("x", 7), Err(libc::EINVAL));
    }

    #[test]
    fn binds_and_connects_below_a_long_directory() {
        // Longer than sun_path: only the parent-relative leaf is in the address.
        let mut directory = std::env::temp_dir();
        directory.push("local-socket-".repeat(10));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("server");
        assert!(path.as_os_str().len() > 104);
        let server = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
        let address = RelativeAddress::open(&path).unwrap();
        let before = std::env::current_dir().unwrap();
        address.bind(server).unwrap();
        assert_eq!(unsafe { libc::listen(server, 1) }, 0);
        let client = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
        address.connect(client).unwrap();
        assert!(path.exists());
        // The thread is back in the process working directory.
        assert_eq!(std::env::current_dir().unwrap(), before);
        unsafe {
            libc::close(client);
            libc::close(server);
        }
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn passes_descriptors_with_data() {
        let (left, right) = UnixDatagram::pair().unwrap();
        use std::os::fd::AsRawFd;
        let file = std::fs::File::open("/dev/null").unwrap();
        send_all(left.as_raw_fd(), b"hi", &[file.as_raw_fd()]).unwrap();
        let mut buffer = [0u8; 8];
        let (count, descriptors) = receive(right.as_raw_fd(), &mut buffer).unwrap();
        assert_eq!(&buffer[..count], b"hi");
        assert_eq!(descriptors.len(), 1);
        assert_ne!(descriptors[0], file.as_raw_fd());
        let flags = unsafe { libc::fcntl(descriptors[0], libc::F_GETFD) };
        assert_eq!(flags & libc::FD_CLOEXEC, libc::FD_CLOEXEC);
        unsafe { libc::close(descriptors[0]) };
    }

    #[test]
    fn long_leaf_is_rejected() {
        let path = std::env::temp_dir().join("s".repeat(120));
        assert_eq!(RelativeAddress::open(&path).err(), Some(libc::ENAMETOOLONG));
    }
}
