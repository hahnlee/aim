use aim_storage::system::ImageLease;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread::JoinHandle,
    time::Duration,
};

const DEADLINE: Duration = Duration::from_secs(5);

struct Peer {
    child: Child,
    events: Receiver<String>,
    output: Option<JoinHandle<()>>,
    dir: PathBuf,
}

impl Peer {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("aim-image-lease-{}-{name}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lease_peer", "--ignored", "--nocapture"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, events) = mpsc::channel();
        let output = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                if let Some(event) = line.strip_prefix("LEASE ") {
                    if sender.send(event.to_string()).is_err() {
                        break;
                    }
                }
            }
        });
        let mut peer = Self {
            child,
            events,
            output: Some(output),
            dir,
        };
        peer.send(&peer.image().display().to_string());
        peer.expect("ready");
        peer
    }

    fn image(&self) -> PathBuf {
        self.dir.join("fixture.dmg")
    }

    fn send(&mut self, command: &str) {
        writeln!(self.child.stdin.as_mut().unwrap(), "{command}").unwrap();
    }

    fn event(&self) -> String {
        self.events
            .recv_timeout(DEADLINE)
            .expect("lease peer stalled")
    }

    fn expect(&self, event: &str) {
        assert_eq!(self.event(), event);
    }

    fn acquire(&mut self, mode: &str) {
        self.send(mode);
        self.expect("attempting");
        self.expect("acquired");
    }

    fn refused_write(&mut self) {
        self.send("write");
        self.expect("attempting");
        let error = self.event();
        assert!(
            error.contains("image is in use; stop its owner before rebuilding"),
            "{error}"
        );
    }

    fn release(&mut self) {
        self.send("release");
        self.expect("released");
    }

    fn finish(&mut self) {
        drop(self.child.stdin.take());
        let deadline = std::time::Instant::now() + DEADLINE;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "lease peer did not exit"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        let _ = self.output.take().unwrap().join();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
#[ignore = "child process driven by the image lease host tests"]
fn lease_peer() {
    let mut input = std::io::stdin().lock().lines();
    let image = PathBuf::from(input.next().unwrap().unwrap());
    let emit = |event: &str| {
        println!("LEASE {event}");
        std::io::stdout().flush().unwrap();
    };
    let mut leases = Vec::new();
    emit("ready");
    for command in input {
        let command = command.unwrap();
        if command == "release" {
            leases.clear();
            emit("released");
            continue;
        }
        emit("attempting");
        let result = match command.as_str() {
            "read" => ImageLease::read(&image),
            "write" => ImageLease::write(&image),
            _ => panic!("unknown lease command: {command}"),
        };
        match result {
            Ok(lease) => {
                leases.push(lease);
                emit("acquired");
            }
            Err(error) => emit(&error),
        }
    }
}

#[test]
fn reader_waits_for_writer_then_protects_the_image() {
    let mut peer = Peer::new("reader-waits");
    let writer = ImageLease::write(&peer.image()).unwrap();
    peer.send("read");
    peer.expect("attempting");
    assert_eq!(
        peer.events.recv_timeout(Duration::from_millis(200)),
        Err(RecvTimeoutError::Timeout),
        "reader must wait while the writer owns the image"
    );
    drop(writer);
    peer.expect("acquired");
    assert!(ImageLease::write(&peer.image()).is_err());
    peer.release();
    drop(ImageLease::write(&peer.image()).unwrap());
    peer.finish();
}

#[test]
fn writer_fails_fast_and_mode_changes_release_the_previous_lease() {
    let mut peer = Peer::new("writer-refuses");
    let reader = ImageLease::read(&peer.image()).unwrap();
    peer.refused_write();
    peer.acquire("read");
    // An attempted upgrade fails without dropping either reader's protection.
    peer.refused_write();
    drop(reader);
    peer.refused_write();
    peer.release();
    peer.acquire("write");
    peer.release();
    peer.acquire("read");
    peer.release();
    drop(ImageLease::write(&peer.image()).unwrap());
    peer.finish();
}
