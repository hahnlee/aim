// Test-only exec wrapper: transport tracing, never linked into the runtime.
// The runtime launcher clears inherited DYLD variables; explicit DARWIN_ART_
// diagnostic configuration reaches this wrapper without changing that policy.
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <unistd.h>

int main(int argc, char** argv) {
  const char* program = std::getenv("DARWIN_ART_SCM_REAL_EXECUTABLE");
  const char* library = std::getenv("DARWIN_ART_SCM_TRACE_LIBRARY");
  if (argc < 1 || !program || program[0] != '/' || !library || library[0] != '/') {
    std::fputs("scm-trace-exec: require absolute real executable/library\n", stderr);
    return 64;
  }
  if (access(program, X_OK) != 0 || access(library, R_OK) != 0) {
    std::fprintf(stderr, "scm-trace-exec: invalid target: %s\n", std::strerror(errno));
    return 66;
  }
  const char* existing = std::getenv("DYLD_INSERT_LIBRARIES");
  if (existing && std::strcmp(existing, library) != 0) {
    std::fputs("scm-trace-exec: refuse to replace existing injection configuration\n", stderr);
    return 64;
  }
  if (setenv("DYLD_INSERT_LIBRARIES", library, 1) != 0 ||
      setenv("DARWIN_ART_TRACE_SCM", "1", 1) != 0) {
    std::fprintf(stderr, "scm-trace-exec: setenv: %s\n", std::strerror(errno));
    return 71;
  }
  // A child re-exec of the actual host must not recursively choose this wrapper.
  argv[0] = const_cast<char*>(program);
  execv(program, argv);
  std::fprintf(stderr, "scm-trace-exec: execv: %s\n", std::strerror(errno));
  return 71;
}
