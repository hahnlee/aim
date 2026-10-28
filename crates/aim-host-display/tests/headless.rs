//! The headless display advertises no native UI and opens no windows.
use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use aim_host_display::{screen, unixsock, wire};
use aim_hostcall::display::{Connect, Windows, mode};

struct Server(Child, std::path::PathBuf);
impl Drop for Server {
    fn drop(&mut self) {
        unsafe { libc::kill(self.0.id() as i32, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.0.try_wait().unwrap().is_none() {
            if Instant::now() > deadline {
                self.0.kill().unwrap();
                self.0.wait().unwrap();
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = std::fs::remove_file(&self.1);
    }
}

fn request(link: &UnixStream, op: u32, id: u64, flag: u32) {
    wire::send(
        link.as_fd(),
        wire::bytes(&wire::Request {
            op,
            id,
            flag,
            ..Default::default()
        }),
        None,
    )
    .unwrap();
}

#[test]
fn headless_display_has_no_windows_or_native_ui() {
    #[link(name = "Metal", kind = "framework")]
    unsafe extern "C" {
        fn MTLCreateSystemDefaultDevice() -> *mut std::ffi::c_void;
    }
    // Buffer import uses the same Metal device as the visible display.
    if unsafe { MTLCreateSystemDefaultDevice() }.is_null() {
        aim_paths::skip("no Metal device for headless scanout buffer import");
        return;
    }
    let socket = std::env::temp_dir().join(format!("aim-headless-{}.sock", std::process::id()));
    let child = Command::new(env!("CARGO_BIN_EXE_aim-display"))
        .args(["--mode", "headless", "--size", "64x48", "--socket"])
        .arg(&socket)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut server = Server(child, socket.clone());
    let mut banner = [0; 32];
    server
        .0
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut banner)
        .unwrap();
    let link: UnixStream = unixsock::connect(&socket).unwrap().into();
    link.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    request(&link, wire::OP_HELLO, wire::VERSION, 0);
    let info = wire::recv_record::<Connect>(link.as_fd()).unwrap().unwrap();
    assert_eq!(
        (info.width, info.height, info.mode),
        (64, 48, mode::HEADLESS)
    );
    assert_eq!(info.vsync_period_ns, 1_000_000_000 / 60);
    for op in [
        wire::OP_WINDOWS,
        wire::OP_NOTIFICATIONS,
        wire::OP_MEDIA,
        wire::OP_SHELL,
        wire::OP_TEXT_INPUT,
    ] {
        let mut ui = UnixStream::connect(&socket).unwrap();
        ui.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        request(&ui, op, wire::VERSION, 0);
        let answer = if op == wire::OP_WINDOWS {
            wire::recv_record::<Windows>(ui.as_fd())
                .unwrap()
                .unwrap()
                .mode
        } else {
            wire::recv_record::<u32>(ui.as_fd()).unwrap().unwrap()
        };
        assert_eq!(answer, mode::HEADLESS);
        assert_eq!(
            ui.read(&mut [0]).unwrap(),
            0,
            "UI bridge remained open: {op}"
        );
    }
    assert_eq!(screen::covered(server.0.id() as i32, 0.0, 0.0), None);
}
