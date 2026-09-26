use super::*;
use darwin_art_build_contract::support_java::production_sources;
use std::collections::BTreeSet;

// Android subsystem support compiler: no fixture source or output dependencies.
pub(crate) fn build_runtime_support_classes(root: &Path) -> Result<()> {
    compile_runtime_support(root).map(|_| ())
}

struct CompiledSupport {
    directory: PathBuf,
    classes: PathBuf,
    inventory: Vec<PathBuf>,
}

fn compile_runtime_support(root: &Path) -> Result<CompiledSupport> {
    let manifest = fs::read_to_string(root.join("runtime/framework/support-sources.txt"))?;
    let sources = production_sources(&manifest)?;
    let build_dir = root.join("_build/runtime-support-java");
    fs::create_dir_all(&build_dir)?;
    // Unique immutable generations: never delete another invocation's output.
    // Downstream packaging must pin the returned generation, not a mutable path.
    let staging = PathBuf::from(
        command_output(
            Command::new("mktemp")
                .arg("-d")
                .arg(build_dir.join("generation.XXXXXX")),
        )?
        .trim(),
    );
    let classes = staging.join("classes");
    let signatures = staging.join("compile-signatures");
    fs::create_dir_all(&classes)?;
    fs::create_dir_all(&signatures)?;
    generate_platform_signatures(root, &staging, &signatures)?;
    let boot = env::join_paths([&signatures])?;
    let mut javac = Command::new(crate::support::support_build_tool("JAVAC", "javac"));
    javac
        .args([
            "-source",
            "8",
            "-target",
            "8",
            "-encoding",
            "UTF-8",
            "-sourcepath",
            "",
            "-bootclasspath",
        ])
        .arg(&boot)
        .arg("-d")
        .arg(&classes);
    for source in &sources {
        javac.arg(root.join(source));
    }
    run_command(&mut javac)?;
    let mut inventory = Vec::new();
    class_inventory(&classes, &classes, &mut inventory)?;
    inventory.sort();
    if inventory.is_empty() {
        return Err("production Java compiler produced no classes".into());
    }
    for class in &inventory {
        let name = class.to_string_lossy();
        if !name.starts_with("dev/darwinart/runtime/") && !name.starts_with("dev/darwinart/system/")
        {
            return Err(format!("unexpected production class: {name}").into());
        }
    }
    fs::write(
        staging.join("class-inventory.txt"),
        inventory
            .iter()
            .map(|path| format!("{}\n", path.display()))
            .collect::<String>(),
    )?;
    // Completion marker is written last. Failed generations are not consumable.
    fs::write(
        staging.join("verified"),
        "production Java ownership verified\n",
    )?;
    println!(
        "build-runtime-support-classes: sources={} classes={} generation={} (no fixture outputs; not yet product DEX)",
        sources.len(),
        inventory.len(),
        staging.display()
    );
    Ok(CompiledSupport {
        directory: staging,
        classes,
        inventory,
    })
}

/// Signature-only class files for the exact pinned runtime: the system
/// process boot classpath plus services.jar. Production owners compile
/// against these, so hidden framework and service APIs keep their real
/// access, generics, nesting and constants.
fn generate_platform_signatures(root: &Path, staging: &Path, output: &Path) -> Result<()> {
    let tool = staging.join("hidden-api-signatures");
    fs::create_dir_all(&tool)?;
    let asm = root.join("_prebuilt/android-16/tools/asm-9.6.jar");
    run_command(
        Command::new(crate::support::support_build_tool("JAVAC", "javac"))
            .args(["--release", "11", "-encoding", "UTF-8", "-classpath"])
            .arg(&asm)
            .arg("-d")
            .arg(&tool)
            .arg(root.join("tools/hidden-api-signatures/HiddenApiSignatures.java")),
    )?;
    let boot = command_output(
        Command::new(crate::support::support_build_tool("PYTHON", "python3"))
            .arg(root.join("tools/bootclasspath/resolve.py")),
    )?;
    // The same JDK as the pinned javac.
    let java =
        PathBuf::from(crate::support::support_build_tool("JAVAC", "javac")).with_file_name("java");
    let mut generator = Command::new(java);
    generator
        .arg("-classpath")
        .arg(env::join_paths([&tool, &asm])?)
        .arg("HiddenApiSignatures")
        .arg(output);
    for jar in env::split_paths(boot.trim()) {
        generator.arg(jar);
    }
    generator.arg(root.join("_build/android16-system-services/services.jar"));
    // The rest of SYSTEMSERVERCLASSPATH (service-art.jar, ...), after
    // services.jar in derive_classpath order.
    for jar in system_server_classpath_jars(root)? {
        generator.arg(jar);
    }
    run_command(&mut generator)
}

/// SYSTEMSERVERCLASSPATH JARs other than services.jar, extracted from the
/// pinned image (upstream/android16-systemserverclasspath.lock).
pub(crate) fn system_server_classpath_jars(root: &Path) -> Result<Vec<PathBuf>> {
    let lock = fs::read_to_string(root.join("upstream/android16-systemserverclasspath.lock"))?;
    let extracted = root.join("_build/android16-systemserverclasspath-original");
    Ok(lock
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            match (fields.next(), fields.next(), fields.next()) {
                (Some(_), Some("classpath"), Some(path))
                    if path != "/system/framework/services.jar" =>
                {
                    Some(extracted.join(path.trim_start_matches('/')))
                }
                _ => None,
            }
        })
        .collect())
}

pub(crate) fn build_runtime_support_dex(root: &Path) -> Result<()> {
    let inspector = build_dex_inspector(root)?;
    let compiled = compile_runtime_support(root)?;
    let dex_dir = compiled.directory.join("dex");
    fs::create_dir_all(&dex_dir)?;
    let mut d8 = Command::new(pinned_path("D8", find_d8)?);
    d8.args(["--min-api", "26"])
        .arg("--lib")
        .arg(pinned_path("PLATFORM", find_android_platform_jar)?)
        .arg("--output")
        .arg(&dex_dir);
    for class in &compiled.inventory {
        d8.arg(compiled.classes.join(class));
    }
    run_command(&mut d8)?;
    // This runtime loads one support DEX. Never silently drop overflow DEXes.
    if fs::read_dir(&dex_dir)?.count() != 1 {
        return Err("production support requires exactly one generated DEX".into());
    }
    let dex = dex_dir.join("classes.dex");
    let output = command_output(Command::new(&inspector).arg(&dex))?;
    verify_production_dex(&output, &compiled.inventory)?;
    let mut corrupt = fs::read(&dex)?;
    *corrupt.last_mut().ok_or("empty support DEX")? ^= 1;
    let corrupt_path = compiled.directory.join("corrupt.dex");
    fs::write(&corrupt_path, corrupt)?;
    let rejected = Command::new(&inspector).arg(&corrupt_path).output()?;
    if rejected.status.success()
        || !String::from_utf8_lossy(&rejected.stderr).contains("DEX verification failed")
    {
        return Err("independent verifier did not reject corrupt support DEX".into());
    }
    let original = fs::read(&dex)?;
    let mut trailing = original.clone();
    trailing.push(0);
    let mut concatenated = original.clone();
    concatenated.extend_from_slice(&original);
    for (name, bytes) in [("trailing", trailing), ("concatenated", concatenated)] {
        let path = compiled.directory.join(format!("invalid-{name}.dex"));
        fs::write(&path, bytes)?;
        let rejected = Command::new(&inspector).arg(&path).output()?;
        if rejected.status.success()
            || !String::from_utf8_lossy(&rejected.stderr).contains("DEX verification failed")
        {
            return Err(format!("independent verifier did not reject {name} DEX").into());
        }
    }
    // Immutable generation can be consumed only after this marker. Product
    // output is published below, never harvested from the fixture compiler.
    fs::write(compiled.directory.join("dex-verified"), &output)?;
    let product_dir = root.join("_build/runtime-support-dex/dex");
    fs::create_dir_all(&product_dir)?;
    let publication = compiled.directory.join("publish.dex");
    fs::copy(&dex, &publication)?;
    write_support_depfile(root)?;
    // Single-file atomic publication, after verification of the pinned input
    // generation. Existing APK readers see either complete old or new bytes.
    fs::rename(&publication, product_dir.join("classes.dex"))?;
    println!(
        "build-runtime-support-dex-independent: generation={} {} corrupt/trailing/concatenated=rejected",
        compiled.directory.display(),
        output.trim()
    );
    Ok(())
}

fn pinned_path(name: &str, fallback: fn() -> Result<PathBuf>) -> Result<PathBuf> {
    env::var_os(format!("DARWIN_ART_SUPPORT_{name}"))
        .map(PathBuf::from)
        .map_or_else(fallback, Ok)
}

fn write_support_depfile(root: &Path) -> Result<()> {
    let Some(input_file) = env::var_os("DARWIN_ART_SUPPORT_INPUTS") else {
        return Ok(());
    };
    let mut inputs: BTreeSet<PathBuf> = fs::read_to_string(input_file)?
        .lines()
        .map(PathBuf::from)
        .collect();
    inputs.insert(root.join(darwin_art_build_contract::support_java::SOURCE_MANIFEST));
    for source in production_sources(&fs::read_to_string(
        root.join(darwin_art_build_contract::support_java::SOURCE_MANIFEST),
    )?)? {
        inputs.insert(root.join(source));
    }
    // The verifier's real compiler dependency closure includes transitive host
    // headers selected by include precedence, not merely guessed AOSP trees.
    collect_verifier_dependencies(&root.join("_build/dex-inspector"), root, &mut inputs)?;
    let escape = |path: &Path| {
        path.to_string_lossy()
            .replace('\\', "\\\\")
            .replace(' ', "\\ ")
            .replace('#', "\\#")
            .replace('$', "$$")
    };
    let output = root.join("_build/runtime-support-dex/dex/classes.dex");
    let depfile = format!(
        "{}: {}\n",
        escape(&output),
        inputs
            .iter()
            .map(|input| escape(input))
            .collect::<Vec<_>>()
            .join(" ")
    );
    fs::write(output.with_extension("dex.d"), depfile)?;
    Ok(())
}

fn collect_verifier_dependencies(
    directory: &Path,
    root: &Path,
    inputs: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    for entry in fs::read_to_string(directory.join("current-depfiles.txt"))?.lines() {
        let path = PathBuf::from(entry);
        let contents = fs::read_to_string(&path)?.replace("\\\n", " ");
        let (_, dependencies) = contents
            .split_once(':')
            .ok_or("invalid verifier compiler depfile")?;
        for word in crate::native_cache::parse_makefile_words(dependencies) {
            let dependency = PathBuf::from(word);
            inputs.insert(if dependency.is_absolute() {
                dependency
            } else {
                root.join(dependency)
            });
        }
    }
    Ok(())
}

fn verify_production_dex(output: &str, inventory: &[PathBuf]) -> Result<()> {
    if !output.starts_with("AOSP DEX: verified=yes version=38 ") {
        return Err(format!("unexpected verified support DEX: {output}").into());
    }
    let definitions: BTreeSet<_> = output
        .split_whitespace()
        .filter_map(|token| {
            token
                .starts_with("class[")
                .then(|| token.split_once('=').map(|(_, name)| name))
                .flatten()
        })
        .collect();
    for name in &definitions {
        if !name.starts_with("Ldev/darwinart/runtime/")
            && !name.starts_with("Ldev/darwinart/system/")
        {
            return Err(format!("support DEX contains non-production definition {name}").into());
        }
    }
    for class in inventory {
        let descriptor = format!("L{};", class.with_extension("").display());
        if !definitions.contains(descriptor.as_str()) {
            return Err(format!("support DEX lost compiled class {descriptor}").into());
        }
    }
    for required in [
        "Ldev/darwinart/runtime/wm/WindowInputPublisher;",
        "Ldev/darwinart/runtime/wm/WindowSurfaceRegistry$RelayoutPublication;",
        "Ldev/darwinart/system/DarwinSystemServer;",
    ] {
        if !definitions.contains(required) {
            return Err(format!("support DEX missing {required}").into());
        }
    }
    verify_service_definitions_external(output)
}

fn class_inventory(root: &Path, directory: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(format!("symlink in compiler output: {}", path.display()).into());
        }
        if kind.is_dir() {
            class_inventory(root, &path, output)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("class") {
            output.push(path.strip_prefix(root)?.to_owned());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verifier_dependency_closure_keeps_actual_host_headers_and_escaped_paths() {
        let directory = PathBuf::from(
            command_output(
                Command::new("mktemp").args(["-d", "/tmp/darwin-art-support-deps.XXXXXX"]),
            )
            .unwrap()
            .trim(),
        );
        let nested = directory.join("foundation/objects");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("object.o.d"), "object.o: compat/unistd.h /opt/homebrew/include/jni.h \\\n path\\ with\\ spaces/header.h\n").unwrap();
        fs::write(
            directory.join("current-depfiles.txt"),
            format!("{}\n", nested.join("object.o.d").display()),
        )
        .unwrap();
        fs::write(
            nested.join("obsolete.o.d"),
            "object.o: /deleted/old-header.h\n",
        )
        .unwrap();
        let mut inputs = BTreeSet::new();
        collect_verifier_dependencies(&directory, &directory, &mut inputs).unwrap();
        assert!(inputs.contains(&directory.join("compat/unistd.h")));
        assert!(inputs.contains(Path::new("/opt/homebrew/include/jni.h")));
        assert!(inputs.contains(&directory.join("path with spaces/header.h")));
        assert!(!inputs.contains(Path::new("/deleted/old-header.h")));
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn support_dex_rejects_contamination_and_missing_nested_definitions() {
        let names = [
            "Ldev/darwinart/runtime/wm/WindowInputPublisher;",
            "Ldev/darwinart/runtime/wm/WindowSurfaceRegistry$RelayoutPublication;",
            "Ldev/darwinart/system/DarwinSystemServer;",
        ];
        let valid = format!(
            "AOSP DEX: verified=yes version=38 classes=3 methods=10 {}",
            names
                .iter()
                .enumerate()
                .map(|(i, name)| format!("class[{i}]={name}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let inventory = vec![PathBuf::from(
            "dev/darwinart/runtime/wm/WindowSurfaceRegistry$RelayoutPublication.class",
        )];
        verify_production_dex(&valid, &inventory).unwrap();
        for forbidden in [
            "Ljavax/microedition/khronos/egl/DarwinEGL10;",
            "Ldev/darwinart/probe/ProbeContext;",
            "Landroid/view/InputChannel;",
            "Lcom/android/server/pm/dex/PackageDexUsage;",
        ] {
            assert!(
                verify_production_dex(&format!("{valid} class[3]={forbidden}"), &inventory)
                    .is_err()
            );
        }
        let missing = vec![PathBuf::from(
            "dev/darwinart/runtime/wm/WindowSurfaceRegistry$NewNested.class",
        )];
        assert!(verify_production_dex(&valid, &missing).is_err());
    }
    #[test]
    fn rejects_fixture_traversal_signatures_duplicates_and_empty_inputs() {
        for manifest in [
            "probes/Hello.java",
            "runtime/framework/../Probe.java",
            "runtime/framework/compile-stubs/android/net/NetworkCapabilities.java",
            "runtime/framework/X.java\nruntime/framework/X.java",
            "# empty",
        ] {
            assert!(production_sources(manifest).is_err(), "{manifest}");
        }
        assert_eq!(
            production_sources("# source\nruntime/framework/wm/WindowInputPublisher.java\n")
                .unwrap()
                .len(),
            1
        );
    }
}
