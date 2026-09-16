#include <cassert>
#include <cstdarg>
#include <cstdint>
#include <cstdio>

extern "C" void* darwin_art_linker_dlsym(void*, const char*);
namespace {
// Android arm64 ANativeWindow prefix through perform, used as a producer spy.
// No display, queue or SurfaceFlinger behavior is emulated by this test.
struct Base { int32_t magic, version; void* reserved[4]; void (*inc)(Base*); void (*dec)(Base*); };
struct Window {
  Base common{};
  uint32_t flags{};
  int32_t min{}, max{};
  float xdpi{}, ydpi{};
  intptr_t oem[4]{};
  int (*swap)(Window*, int){};
  int (*dequeue)(Window*, void**){};
  int (*lock)(Window*, void*){};
  int (*queue)(Window*, void*){};
  int (*query)(const Window*, int, int*){};
  int (*perform)(Window*, int, ...){};
};
double rate;
int compatibility, strategy, calls;
int references;
Base* expected_base;
int query_error;
int last_query;
void Acquire(Base* base) { assert(base == expected_base); ++references; }
void Release(Base* base) { assert(base == expected_base); --references; }
int Dimensions(const Window*, int operation, int* value) {
  last_query = operation;
  assert(operation >= 0 && operation <= 2);
  *value = 123 + operation;
  return query_error;
}
int Query(const Window*, int operation, int* value) { assert(operation == 17); *value = 1; return 0; }
int Perform(Window*, int operation, ...) {
  assert(operation == 40);
  va_list args;
  va_start(args, operation);
  rate = va_arg(args, double);
  compatibility = va_arg(args, int);
  strategy = va_arg(args, int);
  va_end(args);
  ++calls;
  return -38; // Producer error must be propagated, never coerced to success.
}
}
void TestWindowFrameRate(void* library) {
  auto set = reinterpret_cast<int32_t (*)(void*, float, int8_t)>(
      darwin_art_linker_dlsym(library, "ANativeWindow_setFrameRate"));
  auto extended = reinterpret_cast<int32_t (*)(void*, float, int8_t, int8_t)>(
      darwin_art_linker_dlsym(library, "ANativeWindow_setFrameRateWithChangeStrategy"));
  assert(set && extended);
  Window window;
  // No Darwin native-window object is allocated here. Android's owner supplies
  // the vtable; the facade must not read a private registry object's fields.
  expected_base = &window.common;
  references = 1;
  window.common.inc = Acquire;
  window.common.dec = Release;
  window.query = Dimensions;
  auto acquire = reinterpret_cast<void (*)(void*)>(
      darwin_art_linker_dlsym(library, "ANativeWindow_acquire"));
  auto release = reinterpret_cast<void (*)(void*)>(
      darwin_art_linker_dlsym(library, "ANativeWindow_release"));
  assert(acquire && release);
  acquire(&window);
  assert(references == 2);
  release(&window);
  assert(references == 1);
  const char* queries[] = {"ANativeWindow_getWidth", "ANativeWindow_getHeight", "ANativeWindow_getFormat"};
  for (int operation = 0; operation < 3; ++operation) {
    auto query = reinterpret_cast<int32_t (*)(void*)>(
        darwin_art_linker_dlsym(library, queries[operation]));
    assert(query);
    query_error = 0;
    assert(query(&window) == 123 + operation && last_query == operation);
    query_error = -19;
    assert(query(&window) == -19 && last_query == operation);
  }
  std::puts("ANativeWindow base: foreign owner ref callbacks, query values/errors PASS");
  window.query = Query; window.perform = Perform;
  calls = 0;
  assert(set(&window, 24.5f, 1) == -38);
  assert(calls == 1 && rate == 24.5 && compatibility == 1 && strategy == 0);
  assert(extended(&window, 60.0f, 0, 1) == -38);
  assert(calls == 2 && rate == 60.0 && compatibility == 0 && strategy == 1);
  assert(set(nullptr, 30.0f, 1) == -22 && calls == 2);
  window.perform = nullptr;
  assert(extended(&window, 30.0f, 0, 0) == -22 && calls == 2);
  std::puts("ANativeWindow frame rate: promoted arguments/default strategy/producer errors PASS");
}
