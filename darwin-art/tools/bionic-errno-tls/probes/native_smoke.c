#include "darwin_art_bionic_errno.h"

#include <errno.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>

#define CHECK(condition) do { if (!(condition)) return __LINE__; } while (0)

typedef struct ThreadResult {
  int32_t value;
  int host_errno_value;
  int32_t* bionic_address;
  int* host_address;
} ThreadResult;

static _Atomic int kReady;
static _Atomic int kRelease;

static void* Worker(void* raw_result) {
  ThreadResult* result = (ThreadResult*)raw_result;
  errno = result->host_errno_value;
  darwin_art_bionic_errno_store(result->value);
  result->bionic_address = darwin_art_bionic___errno();
  result->host_address = &errno;
  if (*result->bionic_address != result->value || errno != result->host_errno_value)
    result->value = -1;
  atomic_fetch_add_explicit(&kReady, 1, memory_order_release);
  while (atomic_load_explicit(&kRelease, memory_order_acquire) == 0) {
  }
  return NULL;
}

int main(void) {
  errno = 27001;
  CHECK(darwin_art_bionic_errno_load() == 0);
  int32_t output = 777;
  CHECK(darwin_art_bionic_errno_from_darwin(EAGAIN, &output) == 1);
  CHECK(output == 11 && errno == 27001);
  CHECK(darwin_art_bionic_errno_set_from_darwin(ENOTSUP) == 1);
  CHECK(darwin_art_bionic_errno_load() == 95 && errno == 27001);
  int host_output = 779;
  CHECK(darwin_art_bionic_errno_to_darwin(38, &host_output) == 1);
  CHECK(host_output == ENOSYS && errno == 27001);
  CHECK(darwin_art_bionic_errno_load() == 95);
  CHECK(darwin_art_bionic_errno_to_darwin(0x3fffffff, &host_output) == 0);
  CHECK(host_output == ENOSYS && errno == 27001);
  CHECK(darwin_art_bionic_errno_to_darwin(38, NULL) == 0);
  CHECK(darwin_art_bionic_errno_load() == 95);
  output = 778;
  CHECK(darwin_art_bionic_errno_from_darwin(0x3fffffff, &output) == 0);
  CHECK(output == 778 && darwin_art_bionic_errno_load() == 95 && errno == 27001);
  CHECK(darwin_art_bionic_errno_set_from_darwin(0x3fffffff) == 0);
  CHECK(darwin_art_bionic_errno_load() == 95 && errno == 27001);
  errno = 0x3fffffff;
  CHECK(darwin_art_bionic_errno_capture_host() == 0);
  CHECK(darwin_art_bionic_errno_load() == 95 && errno == 0x3fffffff);
  errno = ENOENT;
  CHECK(darwin_art_bionic_errno_capture_host() == 1);
  CHECK(darwin_art_bionic_errno_load() == 2 && errno == ENOENT);
  darwin_art_bionic_errno_publish_result(0);
  CHECK(darwin_art_bionic_errno_load() == 2);
  darwin_art_bionic_errno_publish_result(12);
  CHECK(darwin_art_bionic_errno_load() == 12);
  CHECK(darwin_art_bionic_errno_resolve("__errno") != NULL);
  CHECK(darwin_art_bionic_errno_resolve("errno") == NULL);
  CHECK(darwin_art_bionic_errno_resolve(NULL) == NULL);

  ThreadResult first = {.value = 101, .host_errno_value = 27101};
  ThreadResult second = {.value = 202, .host_errno_value = 27202};
  pthread_t first_thread;
  pthread_t second_thread;
  CHECK(pthread_create(&first_thread, NULL, Worker, &first) == 0);
  CHECK(pthread_create(&second_thread, NULL, Worker, &second) == 0);
  while (atomic_load_explicit(&kReady, memory_order_acquire) != 2) {
  }
  CHECK(first.bionic_address != NULL && second.bionic_address != NULL);
  CHECK(first.bionic_address != second.bionic_address);
  CHECK((void*)first.bionic_address != (void*)first.host_address);
  CHECK((void*)second.bionic_address != (void*)second.host_address);
  atomic_store_explicit(&kRelease, 1, memory_order_release);
  CHECK(pthread_join(first_thread, NULL) == 0);
  CHECK(pthread_join(second_thread, NULL) == 0);
  CHECK(first.value == 101 && second.value == 202);
  CHECK(darwin_art_bionic_errno_load() == 12);
  puts("bionic errno native pthread smoke: PASS");
  return 0;
}
