extern int darwin_art_profman_main(int argc, char** argv);

extern "C" int darwin_art_run_profman(int argc, char** argv) {
  return darwin_art_profman_main(argc, argv);
}
