//! Explicit experimental jar redirect artifact; no global image inputs change.
use aim_android_image::{
    dex::{self, Dex},
    redirect, system_server,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::ExitCode,
};

const USAGE: &str = "android-image-redirect --input JAR --redirects TABLE --targets AIM_JAR --out JAR [--native-services BASELINE_LIST]";
fn calls(jar: &[u8], owner: &str) -> Result<usize, String> {
    let mut count = 0;
    for bytes in system_server::dex_files(jar)? {
        let dex = Dex::parse(bytes)?;
        for class in &dex.classes {
            if class.descriptor != "Lcom/android/server/pm/UserManagerService;"
                && !class
                    .descriptor
                    .starts_with("Lcom/android/server/pm/UserManagerService$")
            {
                continue;
            }
            let mut names = BTreeSet::new();
            for method in dex.members(class)?.1 {
                names.insert(dex.method(method)?.1);
            }
            for name in names {
                for code in dex.methods_named(class, &name)? {
                    let units = dex::units(bytes, &code)?;
                    let mut pc = 0;
                    while pc < units.len() {
                        if matches!(units[pc] & 0xff, 0x6e..=0x72 | 0x74..=0x78) {
                            let index = *units.get(pc + 1).ok_or("truncated invoke instruction")?;
                            if dex.method(u32::from(index))?.0 == owner {
                                count += 1;
                            }
                        }
                        pc += dex::instruction_units(&units, pc)?;
                    }
                }
            }
        }
    }
    Ok(count)
}
fn protected_root(input: &Path) -> Option<PathBuf> {
    input
        .ancestors()
        .skip(1)
        .find(|path| {
            path.join(".identity").is_file()
                || aim_android_image::identity::extraction_identity_path(path).is_file()
        })
        .map(Path::to_owned)
}
fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut options = BTreeMap::new();
    while let Some(flag) = args.next() {
        if ![
            "--input",
            "--redirects",
            "--targets",
            "--out",
            "--native-services",
        ]
        .contains(&flag.as_str())
        {
            return Err(format!("unexpected argument {flag}"));
        }
        let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
        if options.insert(flag.clone(), PathBuf::from(value)).is_some() {
            return Err(format!("{flag} supplied twice"));
        }
    }
    let required = |flag| {
        options
            .get(flag)
            .cloned()
            .ok_or_else(|| format!("{flag} is required"))
    };
    let input = fs::canonicalize(required("--input")?).map_err(|error| error.to_string())?;
    let targets = fs::canonicalize(required("--targets")?).map_err(|error| error.to_string())?;
    let out = required("--out")?;
    let parent = fs::canonicalize(
        out.parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )
    .map_err(|error| error.to_string())?;
    let out = parent.join(out.file_name().ok_or("output jar has no filename")?);
    for source in [&input, &targets] {
        if out == *source || protected_root(source).is_some_and(|root| out.starts_with(root)) {
            return Err("redirect output would modify an input image tree".into());
        }
    }
    if fs::symlink_metadata(&out).is_ok() {
        return Err("output already exists; choose a fresh artifact path".into());
    }
    let redirects = redirect::parse(
        &fs::read_to_string(required("--redirects")?).map_err(|error| error.to_string())?,
    )?;
    let original = fs::read(&input).map_err(|error| error.to_string())?;
    let targets = fs::read(&targets).map_err(|error| error.to_string())?;
    let mut baseline = original.clone();
    if let Some(list) = options.get("--native-services") {
        let classes = system_server::parse_native_services(
            &fs::read_to_string(list).map_err(|error| error.to_string())?,
        )?
        .into_iter()
        .map(|service| service.class)
        .collect::<Vec<_>>();
        let work = parent.join(format!(
            ".redirect-{}-{}.work",
            std::process::id(),
            unsafe { libc::arc4random() }
        ));
        fs::create_dir(&work).map_err(|error| error.to_string())?;
        let result =
            system_server::patch_services_jar(&input, &work.join("services.jar"), &classes)
                .and_then(|()| {
                    fs::read(work.join("services.jar")).map_err(|error| error.to_string())
                });
        let cleanup = fs::remove_dir_all(&work).map_err(|error| error.to_string());
        baseline = match (result, cleanup) {
            (Ok(bytes), Ok(())) => bytes,
            (Err(error), Ok(())) | (Ok(_), Err(error)) => return Err(error),
            (Err(error), Err(cleanup)) => {
                return Err(format!("{error}; owned redirect workspace cleanup: {cleanup}"));
            }
        };
    }
    let redirected =
        redirect::redirect_jar(&baseline, &redirects, &system_server::dex_files(&targets)?)?;
    let um_sites = redirects
        .iter()
        .filter(|entry| {
            entry
                .caller
                .starts_with("com.android.server.pm.UserManagerService")
        })
        .map(|entry| entry.calls)
        .sum::<usize>();
    if um_sites != 0 {
        let before = calls(&original, "Lcom/android/server/pm/PackageManagerService;")?;
        let remaining = calls(&redirected, "Lcom/android/server/pm/PackageManagerService;")?;
        let native = calls(
            &redirected,
            "Lcom/android/server/pm/NativeUserManagerBridge;",
        )?;
        if um_sites != 14 || before != 14 || remaining != 0 || native != 14 {
            return Err(format!(
                "pinned UM redirects differ: rows={um_sites}, original={before}, remaining={remaining}, native={native}"
            ));
        }
    }
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&out)
        .map_err(|error| error.to_string())?;
    let identity = output.metadata().map_err(|error| error.to_string())?;
    if let Err(error) = output
        .write_all(&redirected)
        .and_then(|()| output.sync_all())
    {
        drop(output);
        if fs::symlink_metadata(&out)
            .is_ok_and(|current| current.dev() == identity.dev() && current.ino() == identity.ino())
        {
            fs::remove_file(&out)
                .map_err(|cleanup| format!("write {error}; owned output cleanup {cleanup}"))?;
        }
        return Err(error.to_string());
    }
    println!(
        "{} redirect rows; {} UM calls checked; wrote {}",
        redirects.len(),
        um_sites,
        out.display()
    );
    Ok(())
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}
