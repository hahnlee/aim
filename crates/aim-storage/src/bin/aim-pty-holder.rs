//! Native event-driven PTY endpoint owner (#1259).
use std::{io, path::PathBuf};
fn main() {
    if let Err(error) = run() {
        eprintln!("aim-pty-holder: {error}");
        std::process::exit(1);
    }
}
fn run() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut name = None;
    let mut runtime = None;
    let mut table=None;
    let mut locator=None;
    while let Some(arg) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
        match arg.as_str() {
            "--service-name" => name = Some(value),
            "--runtime" => runtime = Some(PathBuf::from(value)),
            "--identity-table"=>table=Some(PathBuf::from(value)),
            "--owner-locator"=>locator=Some(PathBuf::from(value)),
            _ => return Err(io::Error::from_raw_os_error(libc::EINVAL)),
        }
    }
    let name = name.ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
    let runtime = runtime.ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
    let mut server=aim_storage::pty_owner::transport::Server::register(&name,&runtime)?.with_parent_controller()?;
    if let Some(table)=table{server=server.with_credentials(&table)?;}
    struct Locator{path:PathBuf,identity:(u64,u64)}
    impl Drop for Locator{fn drop(&mut self){use std::os::unix::fs::MetadataExt;match std::fs::symlink_metadata(&self.path){Ok(metadata)if(metadata.dev(),metadata.ino())==self.identity=>{if let Err(error)=std::fs::remove_file(&self.path){eprintln!("PTY locator cleanup: {error}");}},Ok(_)=>eprintln!("PTY locator replaced before cleanup"),Err(error)if error.kind()==io::ErrorKind::NotFound=>{},Err(error)=>eprintln!("PTY locator cleanup: {error}")}}}
    let _locator=if let Some(path)=locator{
        use std::os::unix::fs::MetadataExt;
        let process=aim_storage::process_namespace::ProcessIdentity::running(std::process::id()as i32)?;
        aim_storage::pty_owner::transport::OwnerConfig{endpoint:name,process}.write(&path)?;
        let metadata=std::fs::symlink_metadata(&path)?;
        Some(Locator{path,identity:(metadata.dev(),metadata.ino())})
    }else{None};
    server.run()
}
