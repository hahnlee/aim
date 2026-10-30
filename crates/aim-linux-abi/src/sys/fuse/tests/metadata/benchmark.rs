//! Host-only timings through the app host's daemon and actual FUSE broker.
use super::*;
use aim_emulated_storage::{Daemon, op};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Default, Clone, Debug)]
struct Metrics {
    calls: std::collections::BTreeMap<u32, usize>,
    nanos: std::collections::BTreeMap<u32, u128>,
}

fn timing(label: &str, samples: &mut [f64]) {
    samples.sort_by(f64::total_cmp);
    eprintln!(
        "FUSE_BENCH {label}: mean={:.3} median={:.3} p95={:.3} us n={}",
        samples.iter().sum::<f64>() / samples.len() as f64,
        samples[samples.len() / 2],
        samples[samples.len() * 95 / 100],
        samples.len()
    );
}

#[test]
#[ignore = "host timing benchmark; run explicitly with --ignored --nocapture"]
fn fuse_host_latency_benchmark() {
    let (_guard, root) = crate::vfs::test_view();
    fs::create_dir_all(root.join("root/storage")).unwrap();
    let connection = Connection::new();
    let lower = connection.key.transport().parent().unwrap().join("lower");
    fs::create_dir_all(lower.join("0/Android/data/bench")).unwrap();
    let daemon = Daemon::new(&lower);
    let metrics = Arc::new(Mutex::new(Metrics::default()));
    let recorded = metrics.clone();
    let device = connection.fd;
    let client = Client::new(&connection.key);
    client.mount().unwrap();
    let server = std::thread::spawn(move || {
        loop {
            let packet = next(device);
            let opcode = u32::from_le_bytes(packet[4..8].try_into().unwrap());
            let start = Instant::now();
            let reply = daemon.handle(&packet);
            {
                let mut m = recorded.lock().unwrap();
                *m.calls.entry(opcode).or_default() += 1;
                *m.nanos.entry(opcode).or_default() += start.elapsed().as_nanos();
            }
            if let Some(reply) = reply {
                write_device(device, &reply).unwrap();
            }
            if opcode == op::DESTROY {
                break;
            }
        }
    });
    let mount = "/storage/emulated";
    let options =
        crate::sys::fuse_mount::parse(b"fd=17,rootmode=40000,user_id=0,group_id=0,allow_other")
            .unwrap();
    crate::vfs::add_fuse_mount(
        mount,
        connection.key.transport().to_owned(),
        lower.clone(),
        "fuse",
        &options,
        false,
    )
    .unwrap();
    let base = route(&connection, "0/Android/data/bench");
    let parent = fuse_client::lookup(&base).unwrap();
    // The daemon advertises zero timeouts. Cold means a previously unqueried
    // child; warm repeats that child. Neither flushes the host's filesystem cache.
    for operation in [
        "LOOKUP",
        "GETATTR",
        "ACCESS",
        "MKDIR",
        "STATFS",
        "OPENDIR+READDIR",
    ] {
        let mut cold = Vec::new();
        let mut warm = Vec::new();
        for sample in 0..30 {
            let name = format!("{operation}-{sample}");
            if operation != "MKDIR" {
                fs::create_dir(lower.join(&base.relative).join(&name)).unwrap();
            }
            let mut entry_name = name.as_bytes().to_vec();
            entry_name.push(0);
            let mut node = if matches!(operation, "MKDIR" | "LOOKUP") {
                parent
            } else {
                let reply = client
                    .request(op::LOOKUP, parent, &entry_name, 0, 0, 1)
                    .unwrap();
                u64::from_le_bytes(reply.body[..8].try_into().unwrap())
            };
            for samples in [&mut cold, &mut warm] {
                let start = Instant::now();
                match operation {
                    "LOOKUP" => {
                        let reply = client
                            .request(op::LOOKUP, parent, &entry_name, 0, 0, 1)
                            .unwrap();
                        node = u64::from_le_bytes(reply.body[..8].try_into().unwrap());
                        samples.push(start.elapsed().as_secs_f64() * 1e6);
                        client
                            .request_no_reply(op::FORGET, node, &1u64.to_le_bytes(), 0, 0, 1)
                            .unwrap();
                        continue;
                    }
                    "GETATTR" => {
                        client
                            .request(op::GETATTR, node, &[0; 16], 0, 0, 1)
                            .unwrap();
                    }
                    "ACCESS" => {
                        client.request(op::ACCESS, node, &[0; 8], 0, 0, 1).unwrap();
                    }
                    "MKDIR" => {
                        let mut body = 0o755u32.to_le_bytes().to_vec();
                        body.extend(0u32.to_le_bytes());
                        body.extend(&entry_name);
                        let result = client.request(op::MKDIR, parent, &body, 0, 0, 1);
                        if let Ok(reply) = result {
                            node = u64::from_le_bytes(reply.body[..8].try_into().unwrap());
                        } else {
                            assert_eq!(result.err(), Some(crate::errno::EEXIST));
                        }
                    }
                    "STATFS" => {
                        client.request(op::STATFS, node, &[], 0, 0, 1).unwrap();
                    }
                    _ => {
                        let opened = client.request(op::OPENDIR, node, &[0; 8], 0, 0, 1).unwrap();
                        let mut body = vec![0; 40];
                        body[..8].copy_from_slice(&opened.body[..8]);
                        body[16..20].copy_from_slice(&4096u32.to_le_bytes());
                        client.request(op::READDIR, node, &body, 0, 0, 1).unwrap();
                        samples.push(start.elapsed().as_secs_f64() * 1e6);
                        let mut release = vec![0; 24];
                        release[..8].copy_from_slice(&opened.body[..8]);
                        client
                            .request(op::RELEASEDIR, node, &release, 0, 0, 1)
                            .unwrap();
                        continue;
                    }
                }
                samples.push(start.elapsed().as_secs_f64() * 1e6);
            }
            if operation != "LOOKUP" {
                client
                    .request_no_reply(op::FORGET, node, &1u64.to_le_bytes(), 0, 0, 1)
                    .unwrap();
            }
        }
        timing(&format!("wire/{operation}/cold"), &mut cold);
        timing(&format!("wire/{operation}/warm"), &mut warm);
    }
    let at = crate::vfs::LINUX_AT_FDCWD as u64;
    let access = |path: u64, mode| crate::sys::fs::faccessat(at, path, mode, 0);
    let mkdir = |path| crate::sys::fsops::mkdirat([at, path, 0o777, 0, 0, 0]);
    for operation in [
        "access",
        "stat",
        "mkdir-existing",
        "statfs",
        "external-probe",
    ] {
        let before = metrics.lock().unwrap().calls.clone();
        let mut cold = Vec::new();
        let mut warm = Vec::new();
        for sample in 0..30 {
            let relative = format!("0/Android/data/bench-{operation}-{sample}/files");
            fs::create_dir_all(lower.join(&relative).parent().unwrap()).unwrap();
            if operation != "external-probe" {
                fs::create_dir(lower.join(&relative)).unwrap();
            }
            let path = std::ffi::CString::new(format!("{mount}/{relative}")).unwrap();
            let pointer = path.as_ptr() as u64;
            for samples in [&mut cold, &mut warm] {
                let mut buffer = [0u64; 32];
                let output = buffer.as_mut_ptr() as u64;
                let start = Instant::now();
                let result = match operation {
                    "access" => access(pointer, 0),
                    "stat" => crate::sys::fs::newfstatat([at, pointer, output, 0, 0, 0]),
                    "mkdir-existing" => mkdir(pointer),
                    "statfs" => crate::sys::fs::statfs([pointer, output, 0, 0, 0, 0]),
                    _ => {
                        // Android 16 ContextImpl.ensureExternalDirsExistOrFilter(true)
                        // + File.mkdirs (parent exists) + File.getUsableSpace:
                        // exists; if absent, exists again then mkdir; canWrite; statvfs.
                        if access(pointer, 0) != 0 {
                            assert_eq!(access(pointer, 0), -(crate::errno::ENOENT as i64));
                            assert_eq!(mkdir(pointer), 0);
                        }
                        assert_eq!(access(pointer, 2), 0);
                        crate::sys::fs::statfs([pointer, output, 0, 0, 0, 0])
                    }
                };
                assert_eq!(
                    result,
                    if operation == "mkdir-existing" {
                        -(crate::errno::EEXIST as i64)
                    } else {
                        0
                    }
                );
                samples.push(start.elapsed().as_secs_f64() * 1e6);
            }
        }
        timing(&format!("syscall/{operation}/cold"), &mut cold);
        timing(&format!("syscall/{operation}/warm"), &mut warm);
        let delta: std::collections::BTreeMap<_, _> = metrics
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter_map(|(op, n)| {
                let delta = n - before.get(op).unwrap_or(&0);
                (delta != 0).then_some((*op, delta))
            })
            .collect();
        eprintln!("FUSE_BENCH syscall/{operation} requests={delta:?}");
    }
    fuse_client::forget(&base, parent, 1).unwrap();
    client.request(op::DESTROY, 1, &[], 0, 0, 1).unwrap();
    server.join().unwrap();
    let m = metrics.lock().unwrap();
    for (opcode, count) in &m.calls {
        eprintln!(
            "FUSE_BENCH daemon/{opcode}: mean={:.3} us n={count}",
            m.nanos[opcode] as f64 / *count as f64 / 1000.0
        );
    }
    assert!(crate::vfs::remove_mount(mount).unwrap());
}
