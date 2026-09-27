use android_image_extract::{Result, image, invalid, x18};
use std::io::Write;
use std::path::PathBuf;

const USAGE: &str = "\
usage: android-image-extract ARCHIVE.zip OUTDIR
       android-image-extract identity ARCHIVE.zip OUTDIR
       android-image-extract scan-x18 OUTDIR

Extraction records the archive's sha256, the original's identity, in
OUTDIR.identity beside the tree. `identity` records it for a tree extracted
earlier from ARCHIVE.zip.";

fn run() -> Result<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let mut stdout = std::io::stdout().lock();
    match args.as_slice() {
        [command, root] if command.as_os_str() == "scan-x18" => {
            let scan = x18::scan_tree(root)?;
            x18::print(&scan, &mut stdout)?;
        }
        [command, archive, out] if command.as_os_str() == "identity" => {
            let (identity, path) = image::record_identity(archive, out)?;
            writeln!(stdout, "identity {identity} {}", path.display())?;
        }
        [archive, out] => {
            let lines = image::extract(archive, out)?;
            image::print(&lines, &mut stdout)?;
            let (identity, path) = image::record_identity(archive, out)?;
            writeln!(stdout, "identity {identity} {}", path.display())?;
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
