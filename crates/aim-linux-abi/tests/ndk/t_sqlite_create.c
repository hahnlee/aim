// Exercise the image's original SQLite and bionic, without a guest boot.
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <unistd.h>

typedef struct sqlite3 sqlite3;
static int (*db_open)(const char*, sqlite3**, int, const char*);
static int (*db_exec)(sqlite3*, const char*, int (*)(void*, int, char**, char**), void*, char**);
static int (*db_close)(sqlite3*);
static int (*db_readonly)(sqlite3*, const char*);
static int (*db_timeout)(sqlite3*, int);
static const char* (*db_error)(sqlite3*);
static pthread_barrier_t barrier;
static const char* path = "/data/local/tmp/sqlite-create/persistent.sqlite";
enum { SQLITE_OPEN_READWRITE = 2, SQLITE_OPEN_CREATE = 4 };

static void* worker(void* argument) {
  const char* filename = argument;
  for (int i = 0; i < 200; ++i) {
    // Alternate direct opens and in-memory connections with an attached
    // persistent database, as keystore2 uses. Both paths name the same inode.
    const char* schema = i % 2 ? "persistent" : "main";
    pthread_barrier_wait(&barrier);
    sqlite3* db = NULL;
    int rc = db_open(i % 2 ? ":memory:" : filename, &db, SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE,
                     NULL);
    if (rc == 0 && db_timeout(db, 5000) != 0) abort();
    if (rc == 0 && i % 2) {
      char sql[256];
      snprintf(sql, sizeof(sql), "ATTACH DATABASE '%s' AS persistent", filename);
      rc = db_exec(db, sql, NULL, NULL, NULL);
    }
    if (rc != 0 || db_readonly(db, schema) != 0) {
      printf("FAIL round=%d open=%d readonly=%d error=%s\n", i, rc, db_readonly(db, schema),
             db_error(db));
      fflush(stdout);
      exit(1);
    }
    rc = db_exec(db,
                 i % 2 ? "CREATE TABLE IF NOT EXISTS persistent.version(id INTEGER)"
                       : "CREATE TABLE IF NOT EXISTS version(id INTEGER)",
                 NULL, NULL, NULL);
    if (rc != 0) {
      printf("FAIL round=%d create=%d error=%s\n", i, rc, db_error(db));
      fflush(stdout);
      exit(1);
    }
    if (db_close(db) != 0) abort();
    int fd = open(filename, O_RDWR | O_CLOEXEC);
    struct stat st;
    if (fd < 0 || fstat(fd, &st) || st.st_uid != 1017 || st.st_gid != 1017 ||
        (st.st_mode & 0777) != 0600 || (fcntl(fd, F_GETFL) & O_ACCMODE) != O_RDWR ||
        access(filename, W_OK) || close(fd))
      abort();
    pthread_barrier_wait(&barrier);
  }
  return NULL;
}

int main(void) {
  // The standalone test has no generated linker configuration for APEX libraries.
  const char* icu[] = {"libicuuc.so", "libicui18n.so", "libandroidicu.so"};
  for (unsigned i = 0; i < sizeof(icu) / sizeof(icu[0]); ++i) {
    char library[256];
    snprintf(library, sizeof(library), "/apex/com.android.i18n/lib64/%s", icu[i]);
    if (!dlopen(library, RTLD_NOW | RTLD_GLOBAL)) {
      printf("FAIL dlopen %s: %s\n", library, dlerror());
      return 1;
    }
  }
  void* lib = dlopen("libsqlite.so", RTLD_NOW);
  if (!lib) {
    printf("FAIL dlopen: %s\n", dlerror());
    return 1;
  }
#define LOAD(dst, name)                 \
  do {                                  \
    *(void**)(&dst) = dlsym(lib, name); \
    if (!dst) abort();                  \
  } while (0)
  LOAD(db_open, "sqlite3_open_v2");
  LOAD(db_exec, "sqlite3_exec");
  LOAD(db_close, "sqlite3_close");
  LOAD(db_readonly, "sqlite3_db_readonly");
  LOAD(db_timeout, "sqlite3_busy_timeout");
  LOAD(db_error, "sqlite3_errmsg");
  if (mkdir("/data/local/tmp/sqlite-create", 0700) ||
      chown("/data/local/tmp/sqlite-create", 1017, 1017) || setgid(1017) || setuid(1017)) {
    perror("setup");
    return 1;
  }
  pthread_barrier_init(&barrier, NULL, 3);
  pthread_t a, b;
  if (pthread_create(&a, NULL, worker, (void*)path) ||
      pthread_create(&b, NULL, worker, "/data/sqlite-alias/persistent.sqlite"))
    abort();
  for (int i = 0; i < 200; ++i) {
    pthread_barrier_wait(&barrier);
    pthread_barrier_wait(&barrier);
    if (unlink(path)) abort();
  }
  pthread_join(a, NULL);
  pthread_join(b, NULL);
  pthread_barrier_destroy(&barrier);
  puts("PASS SQLite concurrent create as uid 1017 (200 rounds)");
  return 0;
}
