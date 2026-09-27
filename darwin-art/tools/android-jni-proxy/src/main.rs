use darwin_art_elf_loader::{
    LoadedElf, ResolveError, ResolvedSymbol, SymbolRequest, SymbolResolver,
};
use std::env;
use std::ffi::c_void;
use std::fs;
use std::num::NonZeroUsize;
use std::process::ExitCode;

unsafe extern "C" {
    fn darwin_art_jni_fixture_reset();
    fn darwin_art_jni_fixture_vm() -> *mut c_void;
    fn darwin_art_jni_fixture_passed() -> i32;
    fn darwin_art_jni_fixture_calls_passed() -> i32;
}

struct FixtureResolver;

impl SymbolResolver for FixtureResolver {
    fn resolve(
        &mut self,
        request: SymbolRequest<'_>,
    ) -> Result<Option<ResolvedSymbol>, ResolveError> {
        if request.symbol != "darwin_art_jni_fixture_vm" {
            return Ok(None);
        }
        if request.version.is_some() {
            return Err(ResolveError::Rejected(
                "fixture VM provider must be unversioned".to_owned(),
            ));
        }
        let address = NonZeroUsize::new(darwin_art_jni_fixture_vm as usize)
            .expect("function address is non-zero");
        // SAFETY: the C provider has the Android fixture's no-argument/pointer-return
        // register-only ABI and remains linked for the lifetime of the loaded image.
        Ok(Some(unsafe { ResolvedSymbol::new(address) }))
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os();
    let _program = arguments.next();
    let fixture = arguments.next().ok_or("missing Android JNI fixture")?;
    if arguments.next().is_some() {
        return Err("usage: android-jni-proxy <android-arm64-fixture.so>".into());
    }

    // SAFETY: resets private process-global test state before the fixture can observe it.
    unsafe { darwin_art_jni_fixture_reset() };
    let bytes = fs::read(fixture)?;
    let mut resolver = FixtureResolver;
    let image = LoadedElf::load_with_resolver(&bytes, &mut resolver)?;
    image.run_initializers()?;
    let result = image.call_exported_i32("jni_proxy_fixture_run")?;
    if result != 0x0001_0006 {
        return Err(format!("JNI_OnLoad returned {result:#x}, expected JNI 1.6").into());
    }
    // SAFETY: the fake backend state is read only after the synchronous ELF call returns.
    if unsafe { darwin_art_jni_fixture_passed() } != 1 {
        return Err("fake Darwin backend did not observe the complete JNI path".into());
    }
    if unsafe { darwin_art_jni_fixture_calls_passed() } != 1 {
        return Err(
            "fake Darwin backend did not receive the variadic/va_list/nonvirtual calls".into(),
        );
    }
    println!(
        "android-jni-proxy: PASS ELF->JNI_OnLoad->JavaVM/GetEnv->proxy JNIEnv->fake Darwin backend"
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("android-jni-proxy: {error}");
            ExitCode::from(2)
        }
    }
}
