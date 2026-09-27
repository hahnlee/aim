use android_image_extract::{Result, image, invalid, x18};
use std::path::PathBuf;

const USAGE: &str =
    "usage: android-image-extract ARCHIVE.zip OUTDIR\n       android-image-extract scan-x18 OUTDIR";

fn run() -> Result<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let mut stdout = std::io::stdout().lock();
    match args.as_slice() {
        [command, root] if command.as_os_str() == "scan-x18" => {
            let scan = x18::scan_tree(root)?;
            x18::print(&scan, &mut stdout)?;
        }
        [archive, out] => {
            let lines = image::extract(archive, out)?;
            image::print(&lines, &mut stdout)?;
        }
        _ => return Err(invalid(USAGE)),
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("android-image-extract: {error}");
        std::process::exit(1);
    }
}
