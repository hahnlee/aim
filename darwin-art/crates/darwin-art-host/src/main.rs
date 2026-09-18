fn main() {
    if let Err(error) = darwin_art_host::run_cli() {
        eprintln!("darwin-art-host: {error}");
        std::process::exit(1);
    }
}
