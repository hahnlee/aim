#include <stdarg.h>
#include <syslog.h>

typedef void (*Openlog)(const char*, int, int);
typedef void (*Closelog)(void);
typedef void (*Syslog)(int, const char*, ...);

static Openlog kOpenlog = &openlog;
static Closelog kCloselog = &closelog;
static Syslog kSyslog = &syslog;

int ReadVariadicInt(va_list arguments) {
  return va_arg(arguments, int);
}

int main(void) {
  return kOpenlog == 0 || kCloselog == 0 || kSyslog == 0 || sizeof(va_list) != 32;
}
