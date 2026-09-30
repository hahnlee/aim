//! The hardware cursor end to end, from the host side of the module: a
//! cursor buffer reaches a window host as the pointer's image, its hot spot
//! where the host's mouse rests, and no cursor as the default one.
//!
//! Runs the display server in window mode: run it in a logged-in session.

use std::io::Read;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use aim_host_display::wire::{self, Host, HostInput, Request, host, input};
use aim_host_display::{MODULE, set_server};
use aim_hostcall::display::{Connect, Cursor, FN_CONNECT, FN_CURSOR, FN_IMPORT, Import, Window};

const PAGE: u64 = 16384;

struct Kill(Child);

impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn call<T>(func: u32, args: &mut T) -> i64 {
    // SAFETY: `args` is the function's argument block.
    unsafe { (MODULE.call)(func, args as *mut T as u64, size_of::<T>() as u64) }
}

/// The next [`host::CURSOR`] record a window host reads, and its pixels.
fn next_cursor(sock: &UnixStream) -> (Host, Vec<u8>) {
    loop {
        let mut fds = Vec::new();
        let mut r = Host::default();
        // SAFETY: `Host` is plain old data.
        let buf = unsafe {
            std::slice::from_raw_parts_mut((&mut r as *mut Host).cast(), size_of::<Host>())
        };
        assert!(
            wire::recv(sock.as_fd(), buf, &mut fds).unwrap(),
            "server gone"
        );
        if r.op == host::CURSOR {
            let mut pixels = vec![0; r.id as usize];
            assert!(wire::recv(sock.as_fd(), &mut pixels, &mut fds).unwrap());
            return (r, pixels);
        }
    }
}

#[test]
fn a_cursor_buffer_is_the_window_hosts_cursor() {
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cursor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // sun_path is 104 bytes on Darwin.
    let socket = std::env::temp_dir().join(format!("dc-{}.sock", std::process::id()));
    let mut server = Command::new(env!("CARGO_BIN_EXE_aim-display"))
        .args(["--mode", "windows", "--socket"])
        .arg(&socket)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut banner = [0u8; 32];
    server
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut banner)
        .unwrap();
    let _server = Kill(server);
    set_server(&socket);
    let mut info = Connect::default();
    let fd = call(FN_CONNECT, &mut info);
    assert!(fd >= 0, "connect: {fd}");
    // SAFETY: the module returned a new fd for us.
    let _events = unsafe { OwnedFd::from_raw_fd(fd as i32) };

    // A window host, whose mouse rests at display pixel (100, 200).
    let sock = UnixStream::connect(&socket).unwrap();
    sock.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let hello = Request {
        op: wire::OP_HOST,
        id: wire::VERSION,
        ..Default::default()
    };
    wire::send(sock.as_fd(), wire::bytes(&hello), None).unwrap();
    let named = Host {
        op: host::HELLO,
        id: wire::VERSION,
        window: Window::with_text(0, 0, "test.cursor"),
        ..Default::default()
    };
    wire::send(sock.as_fd(), wire::bytes(&named), None).unwrap();
    let hover = Host {
        op: host::INPUT,
        input: HostInput {
            kind: input::HOVER,
            x: 100.0,
            y: 200.0,
            ..Default::default()
        },
        ..Default::default()
    };
    wire::send(sock.as_fd(), wire::bytes(&hover), None).unwrap();
    // The host's records and the module's requests arrive on separate
    // connections.
    std::thread::sleep(Duration::from_millis(300));

    // The sprite: 16x24 pixels at (90, 180), so its hot spot is (10, 20).
    let (w, h) = (16u32, 24u32);
    let file = dir.join("cursor");
    let mut data = vec![0u8; PAGE as usize];
    for (i, p) in data[..(w * h * 4) as usize].chunks_exact_mut(4).enumerate() {
        p.copy_from_slice(&[i as u8, 0, 0, 0xff]);
    }
    std::fs::write(&file, &data).unwrap();
    let buffer = std::fs::File::open(&file).unwrap();
    let mut import = Import {
        fd: buffer.as_raw_fd(),
        format: 1,
        width: w,
        height: h,
        stride_bytes: w * 4,
        length: PAGE,
        id: 9,
        ..Default::default()
    };
    assert_eq!(call(FN_IMPORT, &mut import), 0);
    let mut cursor = Cursor {
        id: 9,
        x: 90,
        y: 180,
        changed: 1,
        acquire: -1,
    };
    assert_eq!(call(FN_CURSOR, &mut cursor), 0);
    let (r, pixels) = next_cursor(&sock);
    assert_eq!((r.import.width, r.import.height), (w, h));
    assert_eq!((r.input.x, r.input.y), (10.0, 20.0));
    assert_eq!(pixels, data[..(w * h * 4) as usize]);

    // No cursor layer: the default cursor.
    let mut none = Cursor {
        acquire: -1,
        ..Default::default()
    };
    assert_eq!(call(FN_CURSOR, &mut none), 0);
    let (r, pixels) = next_cursor(&sock);
    assert_eq!((r.id, pixels.len()), (0, 0));
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_dir_all(&dir);
}
