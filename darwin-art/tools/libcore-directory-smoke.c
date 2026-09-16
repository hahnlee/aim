#include "../compat/darwin_libcore_filesystem_bridge.h"
#include "bionic-fs-facade/include/darwin_art_bionic_fs.h"

#include <assert.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>

// Keep this smoke portable under strict C11 feature visibility on the Darwin
// SDK while exercising the same process environment capability as runtime.
extern int setenv(const char*, const char*, int);
extern int unsetenv(const char*);

static int32_t guest_errno;
static int directory_closed;
static int directory_index;
static int directory_error;
static int stat_calls;
static const char* const directory_entries[] = {".", "..", "asset.bin"};

static int managed_stat(const char* path, DarwinArtAndroidStat* status) {
  (void)path;
  ++stat_calls;
  memset(status, 0, sizeof(*status));
  status->st_mode = 0100644;
  return 0;
}

static int load_errno(void) { return guest_errno; }

int darwin_art_bionic_fs_statvfs_core(const char* path,
                                      DarwinArtAndroidStatvfs* status) {
  (void)path;
  (void)status;
  guest_errno = ENOSYS;
  return -1;
}

typedef struct TestDirectory {
  int marker;
} TestDirectory;

static TestDirectory managed_directory = {42};

void* darwin_art_bionic_opendir(const char* path) {
  if (strcmp(path, "/guest/missing") == 0) {
    guest_errno = ENOENT;
    return NULL;
  }
  if (strcmp(path, "/tmp/../") == 0) {
    guest_errno = EACCES;
    return NULL;
  }
  assert(strcmp(path, "/guest/directory") == 0);
  directory_index = 0;
  directory_closed = 0;
  directory_error = 0;
  return &managed_directory;
}

DarwinArtAndroidDirent* darwin_art_bionic_readdir(void* directory) {
  static DarwinArtAndroidDirent entry;
  assert(directory == &managed_directory);
  if (directory_error) {
    guest_errno = EIO;
    return NULL;
  }
  if (directory_index >= (int)(sizeof(directory_entries) /
                               sizeof(directory_entries[0]))) {
    // A normal EOF intentionally leaves the facade errno at zero.
    return NULL;
  }
  memset(&entry, 0, sizeof(entry));
  entry.d_ino = (uint64_t)(directory_index + 1);
  entry.d_type = 8;
  strncpy(entry.d_name, directory_entries[directory_index++],
          sizeof(entry.d_name) - 1);
  return &entry;
}

int darwin_art_bionic_closedir(void* directory) {
  assert(directory == &managed_directory);
  directory_closed = 1;
  return 0;
}

int32_t darwin_art_bionic_errno_load(void) { return guest_errno; }
void darwin_art_bionic_errno_store(int32_t value) { guest_errno = value; }

int main(void) {
  // Before a managed provider is installed, the bridge retains normal host
  // DIR behavior for host-only libcore smoke/regression binaries.
  DIR* host_directory = darwin_art_libcore_opendir("/");
  assert(host_directory != NULL);
  assert(darwin_art_libcore_readdir(host_directory) != NULL);
  assert(darwin_art_libcore_closedir(host_directory) == 0);

  darwin_art_libcore_install_filesystem_provider(managed_stat, NULL, NULL,
                                                 load_errno);
  assert(setenv("DARWIN_ART_TEST_FONT", "/system/fonts/Roboto-Regular.ttf", 1) == 0);
  assert(setenv("DARWIN_ART_RUNTIME_HOST_FILES", "/system/fonts/Roboto-Regular.ttf", 1) == 0);
  struct stat font_status;
  assert(darwin_art_libcore_stat("/system/fonts/Roboto-Regular.ttf", &font_status) == 0);
  assert(stat_calls == 1 && S_ISREG(font_status.st_mode));
  unsetenv("DARWIN_ART_TEST_FONT");
  unsetenv("DARWIN_ART_RUNTIME_HOST_FILES");

  // A configured immutable runtime root is the managed host-directory
  // exception used by ICU. It must retain a host DIR handle and not be sent
  // through the guest resolver.
  assert(setenv("ANDROID_I18N_ROOT", "/tmp", 1) == 0);
  DIR* runtime_directory = darwin_art_libcore_opendir("/tmp");
  assert(runtime_directory != NULL);
  assert(darwin_art_libcore_readdir(runtime_directory) != NULL);
  assert(darwin_art_libcore_closedir(runtime_directory) == 0);
  // Lexical containment is insufficient: an escaped path under the same
  // textual prefix must remain a guest request and fail closed.
  errno = 0;
  assert(darwin_art_libcore_opendir("/tmp/../") == NULL);
  assert(errno == EACCES);
  assert(unsetenv("ANDROID_I18N_ROOT") == 0);

  DIR* directory = darwin_art_libcore_opendir("/guest/directory");
  assert(directory == (DIR*)&managed_directory);
  struct dirent* entry = darwin_art_libcore_readdir(directory);
  assert(entry != NULL && strcmp(entry->d_name, ".") == 0);
  entry = darwin_art_libcore_readdir(directory);
  assert(entry != NULL && strcmp(entry->d_name, "..") == 0);
  entry = darwin_art_libcore_readdir(directory);
  assert(entry != NULL && strcmp(entry->d_name, "asset.bin") == 0);
  assert(darwin_art_libcore_readdir(directory) == NULL);
  assert(errno == 0);
  assert(darwin_art_libcore_closedir(directory) == 0);
  assert(directory_closed);

  errno = 0;
  assert(darwin_art_libcore_opendir("/guest/missing") == NULL);
  assert(errno == ENOENT);

  puts("libcore-directory: host fallback/entries/dots/EOF/ENOENT PASS");
}
