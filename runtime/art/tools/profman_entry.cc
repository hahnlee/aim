extern int aim_profman_main(int argc, char** argv);

extern "C" int aim_run_profman(int argc, char** argv) {
  return aim_profman_main(argc, argv);
}
