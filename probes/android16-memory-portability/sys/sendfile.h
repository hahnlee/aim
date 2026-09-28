#ifndef AIM_ANDROID16_MEMORY_SYS_SENDFILE_H_
#define AIM_ANDROID16_MEMORY_SYS_SENDFILE_H_

// Darwin declares sendfile in sys/socket.h rather than a dedicated
// sys/sendfile.h. Memory itself does not call sendfile; this satisfies the
// shared upstream Portability.h include contract without altering it.
#include <sys/socket.h>

#endif  // AIM_ANDROID16_MEMORY_SYS_SENDFILE_H_
