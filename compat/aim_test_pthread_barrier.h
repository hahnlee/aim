#ifndef AIM_TEST_PTHREAD_BARRIER_H_
#define AIM_TEST_PTHREAD_BARRIER_H_

#include <errno.h>
#include <pthread.h>

#if defined(__APPLE__)

typedef struct {
  pthread_mutex_t mutex;
  pthread_cond_t condition;
  unsigned count;
  unsigned waiting;
  unsigned generation;
} pthread_barrier_t;

typedef int pthread_barrierattr_t;

#define PTHREAD_BARRIER_SERIAL_THREAD (-1)

static inline int pthread_barrier_init(pthread_barrier_t* barrier,
                                       const pthread_barrierattr_t*,
                                       unsigned count) {
  if (barrier == NULL || count == 0) return EINVAL;
  int result = pthread_mutex_init(&barrier->mutex, NULL);
  if (result != 0) return result;
  result = pthread_cond_init(&barrier->condition, NULL);
  if (result != 0) {
    pthread_mutex_destroy(&barrier->mutex);
    return result;
  }
  barrier->count = count;
  barrier->waiting = 0;
  barrier->generation = 0;
  return 0;
}

static inline int pthread_barrier_wait(pthread_barrier_t* barrier) {
  int result = pthread_mutex_lock(&barrier->mutex);
  if (result != 0) return result;
  const unsigned generation = barrier->generation;
  if (++barrier->waiting == barrier->count) {
    barrier->waiting = 0;
    ++barrier->generation;
    result = pthread_cond_broadcast(&barrier->condition);
    pthread_mutex_unlock(&barrier->mutex);
    return result == 0 ? PTHREAD_BARRIER_SERIAL_THREAD : result;
  }
  do {
    result = pthread_cond_wait(&barrier->condition, &barrier->mutex);
  } while (result == 0 && generation == barrier->generation);
  pthread_mutex_unlock(&barrier->mutex);
  return result;
}

static inline int pthread_barrier_destroy(pthread_barrier_t* barrier) {
  int result = pthread_cond_destroy(&barrier->condition);
  int mutex_result = pthread_mutex_destroy(&barrier->mutex);
  return result != 0 ? result : mutex_result;
}

#endif  // defined(__APPLE__)
#endif  // AIM_TEST_PTHREAD_BARRIER_H_
