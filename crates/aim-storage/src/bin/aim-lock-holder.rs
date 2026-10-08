//! Internal native kernel worker; its parent is the controller, never a guest.
use aim_storage::{posix_control, process_namespace};
fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let result = (|| {
        if args.len() != 8
            || args[0] != "--service-name"
            || args[2] != "--controller-pid"
            || args[4] != "--controller-seconds"
            || args[6] != "--controller-microseconds"
        {
            return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
        }
        let number = |value: &str| {
            value
                .parse::<u64>()
                .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))
        };
        let pid = number(&args[3])?;
        let controller = process_namespace::ProcessIdentity {
            host_pid: i32::try_from(pid)
                .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?,
            start_seconds: number(&args[5])?,
            start_microseconds: number(&args[7])?,
        };
        posix_control::serve(&args[1], controller)
    })();
    if let Err(error) = result {
        eprintln!("native POSIX lock holder: {error}");
        std::process::exit(1);
    }
}
