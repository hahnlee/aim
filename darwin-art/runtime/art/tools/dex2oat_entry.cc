extern int darwin_art_dex2oat_main(int argc, char** argv);

// Stable C boundary used by the Rust process owner. The AOSP implementation
// remains otherwise unchanged and performs its normal FastExit on success.
extern "C" int darwin_art_run_dex2oat(int argc, char** argv) {
  return darwin_art_dex2oat_main(argc, argv);
}
