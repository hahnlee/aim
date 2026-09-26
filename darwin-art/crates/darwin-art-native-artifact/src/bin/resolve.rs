use darwin_art_native_artifact::{
    ConversionOutcome, ConversionRequest, GraphSelection, Publication,
    prepare_complete_darwin_graph, resolve_directory_graph,
};
use std::env;
use std::path::Path;
use std::process::ExitCode;

fn run() -> Result<(), String> {
    let arguments = env::args_os().collect::<Vec<_>>();
    if arguments.len() != 5 && arguments.len() != 6 {
        return Err(
            "usage: darwin-art-native-resolve APK_SHA256 RUNTIME_ABI ELF_DIRECTORY DARWIN_DIRECTORY [CONVERTER]"
                .to_owned(),
        );
    }
    let apk_sha256 = arguments[1]
        .to_str()
        .ok_or_else(|| "APK SHA-256 is not UTF-8".to_owned())?;
    let runtime_abi = arguments[2]
        .to_str()
        .ok_or_else(|| "runtime ABI is not UTF-8".to_owned())?;
    let elf_directory = Path::new(&arguments[3]);
    let darwin_directory = Path::new(&arguments[4]);
    // With a converter, the installed Android graph is converted once into a
    // complete Darwin graph (or a cached ELF decision) before resolution.
    if let Some(converter) = arguments.get(5).filter(|value| *value != "none") {
        let outcome = prepare_complete_darwin_graph(&ConversionRequest {
            apk_sha256,
            runtime_abi,
            elf_directory,
            cache_directory: darwin_directory,
            converter: Some(Path::new(converter)),
        })
        .map_err(|error| error.to_string())?;
        let (backend, conversion) = match outcome {
            ConversionOutcome::CompleteDarwin {
                publication,
                libraries,
            } => (
                "darwin",
                format!(
                    "{}:{}",
                    match publication {
                        Publication::Published => "published",
                        Publication::Existing => "existing",
                    },
                    libraries.len()
                ),
            ),
            ConversionOutcome::AndroidElf {
                libraries,
                cached_failure,
                reason,
            } => (
                "elf",
                format!(
                    "{}:{reason}:{}",
                    if cached_failure {
                        "cached"
                    } else {
                        "attempted"
                    },
                    libraries.len()
                ),
            ),
        };
        println!("native-convert: PASS native_backend={backend} conversion={conversion}");
    }
    match resolve_directory_graph(apk_sha256, runtime_abi, elf_directory, darwin_directory)
        .map_err(|error| error.to_string())?
    {
        GraphSelection::CompleteDarwin(paths) => println!(
            "native-resolve: PASS backend=darwin libraries={} directory={}",
            paths.len(),
            darwin_directory.display()
        ),
        GraphSelection::AndroidElf(paths) => println!(
            "native-resolve: PASS backend=elf libraries={} directory={}",
            paths.len(),
            elf_directory.display()
        ),
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("native-resolve: {error}");
            ExitCode::from(2)
        }
    }
}
