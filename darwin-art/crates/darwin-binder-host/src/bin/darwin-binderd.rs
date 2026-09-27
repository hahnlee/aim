//! `darwin-binderd --service NAME`
//!
//! Serves the binder driver under the bootstrap name NAME until killed.
//! Guests reach it with `linux-run --binder NAME`.

fn main() {
    let mut args = std::env::args().skip(1);
    let name = match (args.next().as_deref(), args.next()) {
        (Some("--service"), Some(name)) => name,
        _ => {
            eprintln!("usage: darwin-binderd --service NAME");
            std::process::exit(2);
        }
    };
    if let Err(e) = darwin_binder_host::server::Server::start(&name) {
        eprintln!("darwin-binderd: {e}");
        std::process::exit(1);
    }
    loop {
        std::thread::park();
    }
}
