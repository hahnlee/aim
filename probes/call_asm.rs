unsafe extern "C" {
    fn art_darwin_asm_smoke() -> u64;
}

#[unsafe(no_mangle)]
pub extern "C" fn art_darwin_rust_callback() -> u64 {
    41
}

fn main() {
    let result = unsafe { art_darwin_asm_smoke() };
    assert_eq!(result, 42);
    println!("ART Darwin ARM64 assembly result: {result}");
}
