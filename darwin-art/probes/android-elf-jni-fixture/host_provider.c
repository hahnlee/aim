#include <fcntl.h>
#include <unistd.h>

// An ordinary dependency in the test ELF graph. The runner provisions /data
// before loading, and observes this journal outside the production loader.
__attribute__((visibility("default"))) void
darwin_art_fixture_record_lifecycle(int phase) {
  if (phase < 1 || phase > 9) return;
  int fd = open("/data/jni-lifecycle.journal", O_WRONLY | O_CREAT | O_APPEND,
                0600);
  if (fd < 0) return;
  const char record = (char)('0' + phase);
  (void)write(fd, &record, 1);
  (void)close(fd);
}
