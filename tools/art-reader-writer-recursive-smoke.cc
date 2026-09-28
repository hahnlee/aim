// Focused Darwin regression test for ART's ReaderWriterMutex host fallback.
//
// Android permits a thread to acquire a shared rwlock recursively, including
// while a writer is waiting.  Darwin favors waiting writers, so a second
// rdlock by the current reader can block forever.  The child process and alarm
// keep this probe bounded when run against the unadapted host primitive.

#include <atomic>
#include <cstdint>
#include <cerrno>
#include <csignal>
#include <cstdlib>
#include <pthread.h>
#include <sys/wait.h>
#include <sched.h>
#include <unistd.h>

namespace {

pthread_rwlock_t g_lock;
std::atomic<bool> g_writer_started{false};

void* Writer(void*) {
  g_writer_started.store(true, std::memory_order_release);
  const int result = pthread_rwlock_wrlock(&g_lock);
  if (result == 0) {
    pthread_rwlock_unlock(&g_lock);
  }
  return reinterpret_cast<void*>(static_cast<intptr_t>(result));
}

int Child() {
  if (pthread_rwlock_init(&g_lock, nullptr) != 0) {
    return 10;
  }
  if (pthread_rwlock_rdlock(&g_lock) != 0) {
    return 11;
  }

  pthread_t writer;
  if (pthread_create(&writer, nullptr, Writer, nullptr) != 0) {
    pthread_rwlock_unlock(&g_lock);
    return 12;
  }
  // Wait until the writer has started and has had an opportunity to enqueue.
  // The writer cannot complete while this read lease is held.
  while (!g_writer_started.load(std::memory_order_acquire)) {
    sched_yield();
  }
  usleep(50000);

  // This is the operation that must remain non-blocking for Android lock
  // semantics.  An unadapted Darwin rwlock blocks here once the writer waits.
  alarm(1);
  const int nested = pthread_rwlock_rdlock(&g_lock);
  alarm(0);
  if (nested != 0) {
    pthread_rwlock_unlock(&g_lock);
    pthread_rwlock_unlock(&g_lock);
    pthread_join(writer, nullptr);
    return 13;
  }

  pthread_rwlock_unlock(&g_lock);
  pthread_rwlock_unlock(&g_lock);
  pthread_join(writer, nullptr);
  pthread_rwlock_destroy(&g_lock);
  return 0;
}

}  // namespace

int main() {
  const pid_t child = fork();
  if (child < 0) {
    return 20;
  }
  if (child == 0) {
    _exit(Child());
  }

  int status = 0;
  if (waitpid(child, &status, 0) != child) {
    return 21;
  }
  // SIGALRM is the expected result for the unadapted Darwin primitive.  The
  // adapter under test must make the nested lock complete and return 0.
  if (WIFEXITED(status) && WEXITSTATUS(status) == 0) {
    return 0;
  }
  if (WIFSIGNALED(status) && WTERMSIG(status) == SIGALRM) {
    return 1;
  }
  return 22;
}
