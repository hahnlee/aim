#include <syslog.h>

__attribute__((visibility("default"))) void SyslogFixtureRun(void) {
  openlog("GuestTag", LOG_PID | LOG_PERROR, LOG_LOCAL3);
  syslog(LOG_LOCAL7 | LOG_WARNING, "value=%d name=%s fp=%.1f", 17, "android",
         2.5);
  closelog();
}

__attribute__((visibility("default"))) void SyslogFixtureAfterClose(void) {
  syslog(LOG_INFO, "%s", "after-close");
}

__attribute__((visibility("default"))) void SyslogFixtureUnsupported(void) {
  syslog(LOG_INFO, "%1$s", "unsupported-positional");
}
