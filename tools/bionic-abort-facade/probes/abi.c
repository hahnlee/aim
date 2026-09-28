#include <android/set_abort_message.h>
#include <signal.h>
#include <stdlib.h>

_Static_assert(SIGABRT == 6, "Android arm64 SIGABRT value drift");

static void (*abort_signature)(void) = abort;
static void (*set_message_signature)(const char*) = android_set_abort_message;

int main(void) {
  return abort_signature == 0 || set_message_signature == 0;
}
