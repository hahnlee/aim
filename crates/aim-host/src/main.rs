fn main() {
    if let Err(error) = aim_host::run_cli() {
        eprintln!("aim-host: {error}");
        std::process::exit(1);
    }
}
