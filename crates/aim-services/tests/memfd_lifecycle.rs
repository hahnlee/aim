//! Instance shutdown/startup reclaims short-lived memfd backing names (#1008).
mod common {
    pub mod runtime;
}
use common::runtime::{Boot, Data, run};
use std::{
    fs,
    process::Command,
    time::{Duration, Instant},
};

fn ready(boot: &Boot) {
    run(boot.start_command().args(["start", "--windows"]));
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let output = boot
            .command()
            .args(["shell", "getprop", "sys.boot_completed"])
            .output()
            .unwrap();
        if output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "1" {
            break;
        }
        assert!(Instant::now() < deadline, "boot readiness timed out");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "requires derived image, rebuilt aimctl/guest-init/linux-run and pinned NDK"]
fn stop_and_crash_start_reclaim_memfd_backings() {
    let Some(clang) = aim_paths::ndk_clang(35) else {
        aim_paths::skip("pinned NDK missing");
        return;
    };
    let parent = std::env::temp_dir().join(format!("aim-memfd-instance-{}", std::process::id()));
    fs::create_dir(&parent).unwrap();
    let data = Data(parent);
    let source = data.0.join("memfd.c");
    fs::write(&source, "#define _GNU_SOURCE\n#include <sys/mman.h>\n#include <unistd.h>\n#include <stdio.h>\nint main(void){int f=memfd_create(\"aim-lifecycle-probe\",0); if(f<0)return 1; if(write(f,\"probe\",5)!=5)return 2; char p[64],b[6]={0}; snprintf(p,sizeof(p),\"/proc/self/fd/%d\",f); FILE*r=fopen(p,\"r\"); if(!r||fread(b,1,5,r)!=5)return 3; fclose(r); puts(\"ready\"); fflush(stdout); pause(); return 0;}\n").unwrap();
    let program = data.0.join("memfd-probe");
    run(Command::new(clang)
        .args(["-O1", "-Wall", "-Werror", "-o"])
        .arg(&program)
        .arg(&source));
    let boot = Boot::new(aim_paths::root().join("target/release/aimctl"), data.0.join("g"));
    ready(&boot);
    fs::copy(&program, boot.data.join("data/local/tmp/memfd-probe")).unwrap();
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = ChildGuard(
        boot.client(0)
            .arg("/data/local/tmp/memfd-probe")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    use std::io::{BufRead, BufReader};
    let mut line = String::new();
    BufReader::new(child.0.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line.trim(), "ready");
    let before = fs::read_dir(std::env::temp_dir().join("aim-memfd"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect::<Vec<_>>();
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    run(boot.command().arg("stop"));
    let sweep =
        run(Command::new(aim_paths::root().join("target/release/linux-run")).arg("--sweep-memfds"));
    assert_eq!(
        String::from_utf8_lossy(&sweep.stdout).trim(),
        "reclaimed 0 unused memfd backing files",
        "shutdown left unreferenced backings among {} files",
        before.len()
    );
    ready(&boot);
    let state = fs::read_to_string(format!("{}.aimctl/state", boot.data.display())).unwrap();
    let keeper: i32 = state
        .lines()
        .find_map(|s| s.strip_prefix("pid="))
        .unwrap()
        .parse()
        .unwrap();
    let guest = state
        .lines()
        .find_map(|s| s.strip_prefix("guest="))
        .unwrap();
    let binder = format!("--binder dev.aim.guest-init.{guest}.binder");
    let processes = run(Command::new("ps").args(["-axo", "pid=,command="]));
    let owned: Vec<i32> = String::from_utf8_lossy(&processes.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (pid, command) = line.split_once(' ')?;
            (command.trim_start().starts_with(
                aim_paths::root()
                    .join("target/release/linux-run")
                    .to_str()
                    .unwrap(),
            ) && command.contains(boot.data.to_str().unwrap())
                && command.contains(&binder))
            .then(|| pid.parse().unwrap())
        })
        .collect();
    let crashed_backings: Vec<_> = fs::read_dir(std::env::temp_dir().join("aim-memfd"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert!(!crashed_backings.is_empty());
    assert_eq!(unsafe { libc::getpgid(keeper) }, keeper);
    assert_ne!(unsafe { libc::getpgrp() }, keeper);
    assert_eq!(unsafe { libc::killpg(keeper, libc::SIGKILL) }, 0);
    for pid in owned {
        let status = unsafe { libc::kill(pid, libc::SIGKILL) };
        assert!(status == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH));
    }
    run(boot.command().arg("stop"));
    ready(&boot);
    for backing in crashed_backings {
        assert!(
            !backing.exists(),
            "startup retained crashed backing {}",
            backing.display()
        );
    }
}
