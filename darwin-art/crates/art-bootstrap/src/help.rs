pub(crate) fn print_help() {
    println!("ART Darwin bootstrap");
    println!();
    println!("  doctor     verify the native Apple Silicon environment");
    println!("  sync       fetch locked ART subtrees without .git metadata");
    println!("  sync-jit-sources  fetch only the pinned ART compiler and VIXL source closure");
    println!("  probe-asm  compile and execute an ART-derived Mach-O ARM64 entrypoint");
    println!("  probe-pagesize  compile ART's dynamic page-size path for Darwin");
    println!("  build-foundation  build and execute the minimal libartbase archive");
    println!("  build-skia  build upstream Skia CPU raster core and pixel smoke test");
    println!("  build-hwui-canvas  compile the first upstream HWUI Canvas/Paint gate");
    println!("  build-android-graphics-jni  compile the complete Android GraphicsJNI host module");
    println!("  build-hwui-static  compile the complete Android HWUI host core module");
    println!("  build-androidfw  build the complete Android resource framework archive");
    println!("  build-resource-jni  build Android's four complete resource JNI registrars");
    println!("  build-android-util-log  build Android's complete android.util.Log JNI owner");
    println!("  build-android-runtime-host  own ART's process JavaVM for AndroidRuntime callbacks");
    println!("  build-binder-jni  build pinned AOSP Binder/Parcel JNI and RPC boundary archives");
    println!("  build-libcore-linux  build the complete libcore.io.Linux Darwin registrar");
    println!("  build-os-constants  preserve all Android/Linux constants on Darwin");
    println!("  build-unix-filesystem  build Android's complete UnixFileSystem JNI owner");
    println!("  build-openjdkjvm  build ART's complete OpenJDK VM service provider");
    println!("  build-ziparchive-incfs  build Android's incremental ZIP archive variant");
    println!("  build-hostgraphics  build Android's Darwin native-window host module");
    println!("  build-skia-hwui  build the CoreText-free framework Skia closure");
    println!("  build-graphics-codec-modules  build image_io/JPEG/UltraHDR modules");
    println!("  build-libbase  build the complete Android host libbase provider");
    println!("  build-icu-runtime-adapters  verify ART adapters against Android ICU 76");
    println!("  build-graphics-foundations  build Darwin liblog/libcutils archives");
    println!("  build-nativehelper  build Darwin nativehelper host archives");
    println!("  build-ui-types  build the Darwin Android libui-types archive");
    println!("  build-graphics-codecs  build Darwin zlib/libpng/FreeType archives");
    println!("  build-harfbuzz  build the complete Darwin HarfBuzz archive");
    println!("  build-minikin  build the complete Darwin Minikin archive");
    println!("  build-skia-text  raster pinned Roboto through AOSP FreeType and Skia");
    println!("  build-icu  build Android ICU common/i18n/data for Darwin");
    println!("  probe-minikin-shaping  run real Minikin/HarfBuzz/ICU shaping cases");
    println!("  check-text-shaping  audit the full shaping and raster input closure");
    println!("  build-dex  compile AOSP libdexfile and parse a generated classes.dex");
    println!("  build-elf-jni-dex  add the isolated Android ELF JNI fixture class");
    println!("  build-network-dex  add the isolated Android network JNI fixture class");
    println!("  build-button-dex  compile the isolated real android.widget.Button probe");
    println!("  build-runtime-platform  compile ART host platform sources as Mach-O");
    println!("  build-runtime-core  apply Darwin monitor patches and compile runtime core");
    println!("  probe-park  stress Darwin's pthread-backed LockSupport primitive");
    println!("  build-runtime-arm64  generate ABI constants and compile ARM64 context");
    println!("  build-runtime-unwindstack-core  compile pinned AOSP DWARF/ELF unwind engine");
    println!("  build-interpreter-core  compile ART's C++ interpreter implementation");
    println!(
        "  build-nterp-arm64ng  generate and audit the production Darwin ARM64ng Nterp source"
    );
    println!("  build-jit-compiler      compile pinned AOSP ARM64 optimizing compiler");
    println!("  build-jit-libelffile    compile the pinned libelffile/LZMA JIT link closure");
    println!(
        "  build-android16-boot-image  build and atomically publish the AOSP speed boot image"
    );
    println!("  prepare-runtime-common-shadow  stage shared patched ART sources");
    println!("  build-runtime-bootstrap  compile ART Runtime initialization for Darwin");
    println!(
        "  build-runtime-graphics-bootstrap  compile the isolated real-graphics Runtime flavor"
    );
    println!("  build-graphics-foundation  build cached HWUI and GraphicsJNI foundation archives");
    println!("  build-runtime-hwui-probe  compile the cached HWUI animation/tree probe");
    println!(
        "  build-runtime-graphics-phase-probe  compile the cached graphics presentation phase"
    );
    println!("  build-runtime-graphics-input-probe  compile the cached graphics input probe");
    println!("  build-runtime-graphics-state-probe  compile the graphics state owner probe");
    println!(
        "  build-runtime-graphics-session-probe  compile the opaque graphics session ABI probe"
    );
    println!("  build-runtime-jni-acceptance-probe  compile the DEX/JNI acceptance phase");
    println!("  build-runtime-app-bootstrap-probe  compile the detached app class bootstrap phase");
    println!(
        "  build-runtime-app-presentation-probe  compile the detached Activity/window presentation phase"
    );
    println!("  audit-runtime-link  measure the remaining Runtime::Create link closure");
    println!("  audit-runtime-graphics-link  link ART with the strict Android graphics closure");
    println!(
        "  audit-runtime-graphics-link-fast  validate existing graphics artifacts without upstream rebuilds"
    );
    println!(
        "  audit-runtime-graphics-link-incremental  rebuild changed native objects, then rerun cached closure/link checks"
    );
    println!("  audit-graphics-closure  verify the 32-archive Android graphics closure");
    println!("  probe-runtime-dex  launch Java main(String[]) with Android stdout");
    println!("  probe-runtime-elf-jni  load a fixed Android ELF graph and JNI thunks through ART");
    println!("  probe-runtime-network  run Android JNI loopback HTTP through socket/DNS owners");
    println!("  probe-runtime-apk-direct  load a page-aligned STORED APK graph without extraction");
    println!("  probe-window  display the Android View probe in a native NSWindow");
    println!("  probe-runtime-graphics  draw DecorView through Bitmap-backed AOSP Canvas");
    println!("  probe-runtime-graphics-window  display the real Android Canvas frame in NSWindow");
    println!("  probe-runtime-button  draw a real android.widget.Button through AOSP HWUI");
    println!("  probe-runtime-button-window  display the real Button frame in NSWindow");
    println!("  probe-runtime-apk-app  launch a no-native APK through its manifest Activity");
    println!("  probe-runtime-apk-app-window  display the APK Activity frame in NSWindow");
    println!("  probe-runtime-apk-jni-app  load an AOSP APK with JNI_OnLoad and nativeAnswer");
    println!("  probe-runtime-apk-jni-app-window  display the JNI APK Activity in NSWindow");
    println!("  verify-bootclasspath  verify extracted Android 16 core DEX files");
    println!("  all        run every check and probe");
}
