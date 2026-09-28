fn main() {
    if let Err(error) = run() {
        eprintln!("aim-fixture-runner: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().collect::<Vec<_>>();
    if args.len() < 3 || args[1] != "--fixture-image" {
        return Err(
            "fixture runner requires --fixture-image PATH followed by host arguments".into(),
        );
    }
    let image = aim_host::CliExecutionImage {
        path: std::path::PathBuf::from(&args[2]),
        run_symbol: "aim_run_fixture".into(),
        shutdown_symbol: "aim_shutdown_fixture".into(),
    };
    args.drain(1..3);
    // A fixture run is one-shot: its process, not a reusable embedding
    // session, owns runtime teardown.
    args.insert(1, "--fixture-process".into());
    aim_host::run_cli_with_arguments(args, Some(image), Some(&fixture_outcome::observe))
}
mod fixture_outcome;
