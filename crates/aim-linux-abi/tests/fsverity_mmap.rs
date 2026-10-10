//! Actual bionic mmap access reaches the demand verifier and guest SIGBUS handler.
//! Proof is pre-created by the host core; this does not activate ENABLE ioctl.
use std::{fs::{self,File},os::unix::fs::FileExt,path::Path,process::{Command,Stdio},time::{Duration,Instant}};
#[test]
fn original_guest_touch_verifies_pages_and_delivers_sigbus_for_corruption() {run_guest(16384);}
#[test]
fn original_four_kib_elf_faults_composed_authenticated_rewrite_pages() {run_guest(4096);}
fn run_guest(alignment:u64) {
    let Some(clang)=aim_paths::ndk_clang(35) else { aim_paths::skip("NDK clang35 missing");return; };
    let image=aim_paths::original_image();
    if !image.join("apex/com.android.runtime/bin/linker64").exists() { aim_paths::skip("original image missing");return; }
    let dir=Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("fsverity-mmap-{}-{alignment}",std::process::id()));
    assert!(!dir.exists());fs::create_dir_all(dir.join("data")).unwrap();fs::create_dir_all(dir.join("dev")).unwrap();
    let program=dir.join("data/verity-touch");
    let compile=|output:&Path,page_size:u64|{let status=Command::new(&clang).args(["-O1","-fPIE","-pie"]).arg(format!("-Wl,-z,max-page-size={page_size}")).arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/guest/fsverity_mmap.c")).arg("-o").arg(output).status().unwrap();assert!(status.success());};
    let invalid=dir.join("data/invalid-packed");
    if alignment==4096{compile(&invalid,4096);}
    compile(&program,16384);
    if alignment==4096 {
        // A weaker declared alignment is valid: all actual LOAD addresses and
        // offsets retain the compiler's 16KiB layout and GNU RELRO isolation.
        let mut bytes=fs::read(&program).unwrap();let header=aim_linux_abi::elf::parse_header(&bytes).unwrap();let segments=aim_linux_abi::elf::parse_phdrs(&bytes,&header).unwrap();
        let relro=segments.iter().find(|segment|segment.p_type==0x6474e552).expect("GNU RELRO must remain enabled");
        for(index,segment)in segments.iter().enumerate().filter(|(_,segment)|segment.p_type==aim_linux_abi::elf::PT_LOAD){
            assert_eq!(segment.p_offset%16384,segment.p_vaddr%16384);
            if segment.p_flags&aim_linux_abi::elf::PF_X!=0{assert!((segment.p_vaddr+segment.p_memsz+16383)&!16383<=relro.p_vaddr&!16383||((relro.p_vaddr+relro.p_memsz+16383)&!16383)<=segment.p_vaddr&!16383,"CODE and GNU RELRO must occupy distinct physical pages");}
            let offset=header.e_phoff as usize+index*56+48;bytes[offset..offset+8].copy_from_slice(&4096u64.to_le_bytes());
        }
        fs::write(&program,bytes).unwrap();
    }
    fs::write(dir.join("data/verity-data"),vec![37;32768]).unwrap();let data=File::open(dir.join("data/verity-data")).unwrap();
    let data_identity=aim_storage::fsverity::Identity::from_fd(std::os::fd::AsFd::as_fd(&data)).unwrap();
    let proofroot=fs::canonicalize(&dir).unwrap().join("proof");let store=aim_storage::fsverity::Store::new(&proofroot,&dir.join("fs-verity-leases")).unwrap();
    let admission=store.lock_inode(&data).unwrap();let guard=admission.begin_enable().unwrap();drop(admission);
    let prepared=guard.build(aim_storage::fsverity::BuildOptions::new(1,4096,vec![],16384,4096).unwrap(),&[],||false).unwrap();drop(guard.commit(prepared).unwrap());drop(data);
    let executable=File::open(&program).unwrap();let executable_identity=aim_storage::fsverity::Identity::from_fd(std::os::fd::AsFd::as_fd(&executable)).unwrap();
    let translation=aim_linux_abi::xlate::translate(&fs::read(&program).unwrap(),&aim_linux_abi::xlate::Options::default()).unwrap();if alignment==16384{assert!(matches!(translation.outcome,aim_linux_abi::xlate::Outcome::Translated(_)),"owned TLS/MRS ELF must exercise authenticated derivative execution");}else{assert!(matches!(translation.outcome,aim_linux_abi::xlate::Outcome::Unsupported(_)),"4KiB ELF must exercise authenticated composed rewrite");}
    let admission=store.lock_inode(&executable).unwrap();let enable=admission.begin_enable().unwrap();drop(admission);
    let prepared=enable.build(aim_storage::fsverity::BuildOptions::new(1,4096,vec![],16384,4096).unwrap(),&[],||false).unwrap();drop(enable.commit(prepared).unwrap());drop(executable);
    let attacker=File::options().write(true).open(dir.join("data/verity-data")).unwrap();attacker.write_all_at(&[99],16384+20).unwrap();drop(attacker);
    let root=fs::canonicalize(&dir).unwrap();let mut locator=b"AIMVRTROOT01\0".to_vec();locator.extend_from_slice(proofroot.as_os_str().as_encoded_bytes());fs::write(root.join("fs-verity-root"),locator).unwrap();
    let control_root=Path::new("/tmp").join(format!("aim-verity-{}-{alignment}",std::process::id()));
    assert!(!control_root.exists());fs::create_dir(&control_root).unwrap();let endpoint=control_root.join("ctl");
    let owner=aim_storage::verity_control::RunningServer::start(&endpoint,Duration::from_secs(5)).unwrap();
    aim_storage::verity_control::OwnerConfig{endpoint:fs::canonicalize(&endpoint).unwrap(),process:owner.process}.write(&root.join("verity-control-owner")).unwrap();
    fs::write(root.join("path-map"),format!("root\t/\t{}\nrw\t/data\t{}/data\nrw\t/dev\t{}/dev\n",image.display(),root.display(),root.display())).unwrap();
    if alignment==4096{
        let rejected=Command::new(env!("CARGO_BIN_EXE_linux-run")).args(["--no-cache","--path-map"]).arg(root.join("path-map")).arg("/data/invalid-packed").output().unwrap();
        assert!(!rejected.status.success());assert!(String::from_utf8_lossy(&rejected.stderr).contains("Linux errno 22"),"{}",String::from_utf8_lossy(&rejected.stderr));
    }
    let mut child=Command::new(env!("CARGO_BIN_EXE_linux-run")).args(["--no-cache","--path-map"]).arg(root.join("path-map")).arg("/data/verity-touch").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let deadline=Instant::now()+Duration::from_secs(60);
    while child.try_wait().unwrap().is_none() { if Instant::now()>=deadline {child.kill().unwrap();child.wait().unwrap();panic!("owned guest verity probe timed out");}std::thread::sleep(Duration::from_millis(20)); }
    let output=child.wait_with_output().unwrap();assert!(output.status.success(),"{}\n{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("VERITY_MMAP_SIGBUS_ADDRESS_COW_PASS"));
    let cache=root.join("fs-verity-page-cache");
    let pages:Vec<_>=fs::read_dir(&cache).unwrap().map(|entry|entry.unwrap().path()).filter(|path|path.extension().is_none()&&path.file_name().unwrap().to_string_lossy().starts_with(&data_identity.name())).collect();
    let executable_pages:Vec<_>=fs::read_dir(&cache).unwrap().map(|entry|entry.unwrap().path()).filter(|path|path.extension().is_none()&&path.file_name().unwrap().to_string_lossy().starts_with(&format!("{}-derived-",executable_identity.name()))).collect();
    assert!(!executable_pages.is_empty(),"actual guest execution must fault authenticated derivative pages");
    assert!(executable_pages.iter().all(|path|&fs::read(path).unwrap()[..8]==b"AIMVPG02"));
    assert_eq!(pages.len(),1,"all parent/child shared reads use one inode-bound verified page cache entry");
    assert_eq!(fs::metadata(&pages[0]).unwrap().len(),32768);
    assert!(fs::read(&pages[0]).unwrap()[16384..].iter().all(|byte|*byte==37),"private COW must not change shared cache bytes");
    File::options().write(true).open(&program).unwrap().write_all_at(&[99],0).unwrap();
    let rejected=Command::new(env!("CARGO_BIN_EXE_linux-run")).args(["--no-cache","--path-map"]).arg(root.join("path-map")).arg("/data/verity-touch").output().unwrap();
    assert!(!rejected.status.success(),"corrupt protected ELF must not execute");
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("cannot execute (errno 5)"),"{}",String::from_utf8_lossy(&rejected.stderr));
    owner.shutdown().unwrap();fs::remove_dir(control_root).unwrap();
    fs::remove_dir_all(root).unwrap();
}
