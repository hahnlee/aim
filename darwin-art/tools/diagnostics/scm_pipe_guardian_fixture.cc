// Private macOS SCM_RIGHTS/pipe-guardian feasibility fixture. Not production.
#include <libproc.h>
#include <sys/proc_info.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <poll.h>
#include <unistd.h>

#include <cerrno>
#include <cstdio>
#include <cstring>
#include <vector>

namespace {

constexpr int kObserveMs = 100;
constexpr size_t kMaxRights = 4;

void CloseFd(int *fd) {
  if (*fd >= 0) {
    close(*fd);
    *fd = -1;
  }
}

bool Check(bool condition, const char *what) {
  if (!condition) std::fprintf(stderr, "FAIL %s errno=%d\n", what, errno);
  return condition;
}

int FdCount() {
  errno = 0;
  const int bytes = proc_pidinfo(getpid(), PROC_PIDLISTFDS, 0, nullptr, 0);
  if (bytes <= 0 || bytes % static_cast<int>(sizeof(proc_fdinfo)) != 0)
    return -1;
  std::vector<proc_fdinfo> rows(bytes / sizeof(proc_fdinfo) + 16);
  const int actual = proc_pidinfo(getpid(), PROC_PIDLISTFDS, 0, rows.data(),
                                 static_cast<int>(rows.size() * sizeof(proc_fdinfo)));
  if (actual < 0 || actual % sizeof(proc_fdinfo) != 0) return -1;
  return actual / static_cast<int>(sizeof(proc_fdinfo));
}

enum class EofState { kEof, kOpen, kTimeout, kError };

EofState ObserveGuardian(int read_fd) {
  pollfd descriptor{read_fd, POLLIN | POLLHUP | POLLERR, 0};
  const int result = poll(&descriptor, 1, kObserveMs);
  if (result == 0) return EofState::kTimeout;
  if (result < 0) return EofState::kError;
  char byte = 0;
  const ssize_t read_result = read(read_fd, &byte, 1);
  if (read_result == 0) return EofState::kEof;
  if (read_result < 0 && (errno == EAGAIN || errno == EWOULDBLOCK))
    return EofState::kOpen;
  return read_result > 0 ? EofState::kOpen : EofState::kError;
}

bool AssertNotEof(int read_fd, const char *label) {
  const EofState state = ObserveGuardian(read_fd);
  std::printf("%s before-consume=%s\n", label,
              state == EofState::kTimeout ? "timeout-no-observation" :
              state == EofState::kOpen ? "open" :
              state == EofState::kEof ? "EOF-unexpected" : "error");
  return Check(state != EofState::kEof && state != EofState::kError, label);
}

bool AssertEof(int read_fd, const char *label) {
  const EofState state = ObserveGuardian(read_fd);
  std::printf("%s after-consume=%s\n", label,
              state == EofState::kEof ? "EOF" :
              state == EofState::kTimeout ? "timeout" :
              state == EofState::kOpen ? "open" : "error");
  return Check(state == EofState::kEof, label);
}

bool SendRights(int carrier, const int *fds, size_t count) {
  if (count == 0 || count > kMaxRights) return false;
  char byte = 'G';
  iovec vector{&byte, 1};
  alignas(cmsghdr) unsigned char control[CMSG_SPACE(sizeof(int) * kMaxRights)]{};
  msghdr message{};
  message.msg_iov = &vector;
  message.msg_iovlen = 1;
  message.msg_control = control;
  message.msg_controllen = CMSG_SPACE(sizeof(int) * count);
  cmsghdr *header = CMSG_FIRSTHDR(&message);
  if (!header) return false;
  header->cmsg_level = SOL_SOCKET;
  header->cmsg_type = SCM_RIGHTS;
  header->cmsg_len = CMSG_LEN(sizeof(int) * count);
  std::memcpy(CMSG_DATA(header), fds, sizeof(int) * count);
  return sendmsg(carrier, &message, 0) == 1;
}

bool ReceiveRights(int carrier, int flags, bool with_control,
                   int *message_flags, int *fds, size_t *count) {
  char byte = 0;
  iovec vector{&byte, 1};
  alignas(cmsghdr) unsigned char control[CMSG_SPACE(sizeof(int) * kMaxRights)]{};
  msghdr message{};
  message.msg_iov = &vector;
  message.msg_iovlen = 1;
  if (with_control) {
    message.msg_control = control;
    message.msg_controllen = sizeof(control);
  }
  if (recvmsg(carrier, &message, flags) != 1) return false;
  *message_flags = message.msg_flags;
  *count = 0;
  if (!with_control) return true;
  for (cmsghdr *header = CMSG_FIRSTHDR(&message); header != nullptr;
       header = CMSG_NXTHDR(&message, header)) {
    if (header->cmsg_level != SOL_SOCKET || header->cmsg_type != SCM_RIGHTS ||
        header->cmsg_len < CMSG_LEN(0))
      continue;
    const size_t payload = header->cmsg_len - CMSG_LEN(0);
    if (payload % sizeof(int) != 0) return false;
    const size_t available = payload / sizeof(int);
    if (*count + available > kMaxRights) return false;
    std::memcpy(fds + *count, CMSG_DATA(header), payload);
    *count += available;
  }
  std::printf("receive flags=%#x output_flags=%#x count=%zu rights=", flags,
              message.msg_flags, *count);
  for (size_t index = 0; index < *count; ++index)
    std::printf("%s%d", index ? "," : "", fds[index]);
  std::printf("\n");
  return true;
}

bool MakePair(int pair[2]) { return socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0; }

bool RetainedPayloadCase() {
  int carrier[2] = {-1, -1}, payload[2] = {-1, -1}, guardian[2] = {-1, -1};
  bool ok = MakePair(carrier) && MakePair(payload) && pipe(guardian) == 0;
  if (!Check(ok, "retained setup")) return false;
  int retained = dup(payload[0]);
  const int sent[2] = {payload[0], guardian[1]};
  ok = retained >= 0 && SendRights(carrier[0], sent, 2);
  CloseFd(&payload[0]);
  CloseFd(&guardian[1]);
  ok = ok && AssertNotEof(guardian[0], "retained queued guardian");
  usleep(100000);
  int received[4]{};
  int flags = 0;
  size_t received_count = 0;
  ok = ok && ReceiveRights(carrier[1], 0, true, &flags, received,
                           &received_count) && received_count == 2;
  CloseFd(&carrier[0]);
  CloseFd(&carrier[1]);
  if (received_count == 2) {
    CloseFd(&received[1]);
    ok = ok && AssertEof(guardian[0], "retained imported guardian");
    // Retire the protecting lease only after EOF, then prove the imported
    // endpoint itself remains usable without that extra reference.
    CloseFd(&retained);
    const char byte = 'P';
    char returned = 0;
    ok = ok && write(payload[1], &byte, 1) == 1 && read(received[0], &returned, 1) == 1 &&
         returned == byte;
    CloseFd(&received[0]);
  }
  CloseFd(&retained);
  CloseFd(&payload[1]);
  CloseFd(&guardian[0]);
  return ok;
}

bool PeekCase() {
  int carrier[2] = {-1, -1}, payload[2] = {-1, -1}, guardian[2] = {-1, -1};
  bool ok = MakePair(carrier) && MakePair(payload) && pipe(guardian) == 0;
  const int sent[2] = {payload[0], guardian[1]};
  ok = ok && SendRights(carrier[0], sent, 2);
  CloseFd(&payload[0]);
  CloseFd(&guardian[1]);
  int peeked[4]{}, flags = 0;
  size_t count = 0;
  const int before_peek = FdCount();
  ok = ok && ReceiveRights(carrier[1], MSG_PEEK, true, &flags, peeked, &count) && count == 2;
  // Observed Darwin PEEK does not externalize rights: its control integers
  // are zeros, not newly owned descriptors. Never close them (fd0 is stdin).
  // This proves native queue retention only, not Android peek compatibility.
  ok = ok && FdCount() == before_peek;
  ok = ok && AssertNotEof(guardian[0], "peek queued guardian");
  int consumed[4]{};
  count = 0;
  ok = ok && ReceiveRights(carrier[1], 0, true, &flags, consumed, &count) && count == 2;
  if (count == 2) { CloseFd(&consumed[0]); CloseFd(&consumed[1]); }
  ok = ok && AssertEof(guardian[0], "peek consumed guardian");
  CloseFd(&carrier[0]); CloseFd(&carrier[1]); CloseFd(&payload[1]); CloseFd(&guardian[0]);
  return ok;
}

bool DiscardCase() {
  int carrier[2] = {-1, -1}, payload[2] = {-1, -1}, guardian[2] = {-1, -1};
  bool ok = MakePair(carrier) && MakePair(payload) && pipe(guardian) == 0;
  const int sent[2] = {payload[0], guardian[1]};
  ok = ok && SendRights(carrier[0], sent, 2);
  CloseFd(&payload[0]); CloseFd(&guardian[1]);
  int flags = 0, ignored[1]{}; size_t count = 0;
  ok = ok && ReceiveRights(carrier[1], 0, false, &flags, ignored, &count);
  ok = ok && AssertEof(guardian[0], "discard guardian");
  CloseFd(&carrier[0]); CloseFd(&carrier[1]); CloseFd(&payload[1]); CloseFd(&guardian[0]);
  return ok;
}

bool CarrierCloseCase() {
  int carrier[2] = {-1, -1}, payload[2] = {-1, -1}, guardian[2] = {-1, -1};
  bool ok = MakePair(carrier) && MakePair(payload) && pipe(guardian) == 0;
  const int sent[2] = {payload[0], guardian[1]};
  ok = ok && SendRights(carrier[0], sent, 2);
  CloseFd(&payload[0]); CloseFd(&guardian[1]); CloseFd(&carrier[1]);
  ok = ok && AssertEof(guardian[0], "carrier-close queued guardian");
  CloseFd(&carrier[0]); CloseFd(&payload[1]); CloseFd(&guardian[0]);
  return ok;
}

bool PlainReadDiscardCase() {
  int carrier[2] = {-1, -1}, payload[2] = {-1, -1}, guardian[2] = {-1, -1};
  bool ok = MakePair(carrier) && MakePair(payload) && pipe(guardian) == 0;
  const int sent[2] = {payload[0], guardian[1]};
  ok = ok && SendRights(carrier[0], sent, 2);
  CloseFd(&payload[0]); CloseFd(&guardian[1]);
  char byte = 0;
  ok = ok && read(carrier[1], &byte, 1) == 1 && byte == 'G';
  ok = ok && AssertEof(guardian[0], "plain-read discarded guardian");
  CloseFd(&carrier[0]); CloseFd(&carrier[1]);
  CloseFd(&payload[1]); CloseFd(&guardian[0]);
  return ok;
}

bool FullControlDiscardCase() {
  int carrier[2] = {-1, -1}, payload[2] = {-1, -1}, guardian[2] = {-1, -1};
  bool ok = MakePair(carrier) && MakePair(payload) && pipe(guardian) == 0;
  const int sent[2] = {payload[0], guardian[1]};
  ok = ok && SendRights(carrier[0], sent, 2);
  CloseFd(&payload[0]); CloseFd(&guardian[1]);
  int received[4]{}, flags = 0;
  size_t count = 0;
  ok = ok && ReceiveRights(carrier[1], 0, true, &flags, received, &count) && count == 2;
  // Simulate zero guest control capacity only AFTER full native intake.
  // Every materialized descriptor is explicitly disposed, including guardian.
  for (size_t index = 0; index < count; ++index) CloseFd(&received[index]);
  ok = ok && AssertEof(guardian[0], "full-control manual discard guardian");
  CloseFd(&carrier[0]); CloseFd(&carrier[1]);
  CloseFd(&payload[1]); CloseFd(&guardian[0]);
  return ok;
}

bool ChildSenderCase() {
  int carrier[2] = {-1, -1}, payload[2] = {-1, -1}, guardian[2] = {-1, -1};
  bool ok = MakePair(carrier) && MakePair(payload) && pipe(guardian) == 0;
  pid_t child = ok ? fork() : -1;
  if (child == 0) {
    close(carrier[1]); close(payload[1]); close(guardian[0]);
    const int sent[2] = {payload[0], guardian[1]};
    _exit(SendRights(carrier[0], sent, 2) ? 0 : 1);
  }
  if (!Check(child > 0, "child sender fork")) return false;
  CloseFd(&carrier[0]); CloseFd(&payload[0]); CloseFd(&guardian[1]);
  int status = 0; waitpid(child, &status, 0);
  ok = ok && WIFEXITED(status) && WEXITSTATUS(status) == 0;
  ok = ok && AssertNotEof(guardian[0], "child sender queued guardian");
  int received[4]{}, flags = 0; size_t count = 0;
  ok = ok && ReceiveRights(carrier[1], 0, true, &flags, received, &count) && count == 2;
  if (count == 2) { CloseFd(&received[0]); CloseFd(&received[1]); }
  ok = ok && AssertEof(guardian[0], "child sender consumed guardian");
  CloseFd(&carrier[1]); CloseFd(&payload[1]); CloseFd(&guardian[0]);
  return ok;
}

bool ChildReceiverCase() {
  int carrier[2] = {-1, -1}, payload[2] = {-1, -1}, guardian[2] = {-1, -1};
  bool ok = MakePair(carrier) && MakePair(payload) && pipe(guardian) == 0;
  pid_t child = ok ? fork() : -1;
  if (child == 0) {
    close(carrier[0]); close(payload[1]); close(guardian[0]);
    close(payload[0]); close(guardian[1]);
    int sent[2]{}; int flags = 0; size_t count = 0;
    const bool received = ReceiveRights(carrier[1], 0, true, &flags, sent, &count) && count == 2;
    // Leave delivered descriptors live: _exit kernel teardown is the actual
    // retirement trigger, not explicit receiver cleanup before death.
    close(carrier[1]); _exit(received ? 0 : 1);
  }
  if (!Check(child > 0, "child receiver fork")) return false;
  const int sent[2] = {payload[0], guardian[1]};
  ok = ok && SendRights(carrier[0], sent, 2);
  CloseFd(&payload[0]); CloseFd(&guardian[1]); CloseFd(&carrier[0]);
  int status = 0; waitpid(child, &status, 0);
  ok = ok && WIFEXITED(status) && WEXITSTATUS(status) == 0;
  ok = ok && AssertEof(guardian[0], "child receiver guardian");
  CloseFd(&carrier[1]); CloseFd(&payload[1]); CloseFd(&guardian[0]);
  return ok;
}

bool CtruncCase() {
  int carrier[2] = {-1, -1}, payload[2] = {-1, -1}, guardian[2] = {-1, -1};
  bool ok = MakePair(carrier) && MakePair(payload) && pipe(guardian) == 0;
  const int sent[2] = {payload[0], guardian[1]};
  ok = ok && SendRights(carrier[0], sent, 2);
  CloseFd(&payload[0]); CloseFd(&guardian[1]);
  char byte = 0; iovec vector{&byte, 1};
  unsigned char tiny[sizeof(cmsghdr) - 1]{};
  msghdr message{}; message.msg_iov = &vector; message.msg_iovlen = 1;
  message.msg_control = tiny; message.msg_controllen = sizeof(tiny);
  const ssize_t result = recvmsg(carrier[1], &message, 0);
  std::printf("ctrunc partial result=%zd flags=%#x ctrunc=%s\n", result,
              message.msg_flags, (message.msg_flags & MSG_CTRUNC) ? "yes" : "no");
  ok = ok && result == 1 && (message.msg_flags & MSG_CTRUNC) != 0;
  ok = ok && AssertEof(guardian[0], "ctrunc discarded guardian");
  CloseFd(&carrier[0]); CloseFd(&carrier[1]); CloseFd(&payload[1]); CloseFd(&guardian[0]);
  return ok;
}

bool RunCase(const char *name, bool (*test)(), int baseline) {
  const bool result = test();
  const int after = FdCount();
  const bool balanced = baseline >= 0 && after == baseline;
  std::printf("case=%s result=%s fd_baseline=%d fd_after=%d balanced=%s\n", name,
              result && balanced ? "PASS" : "FAIL", baseline, after,
              balanced ? "yes" : "no");
  return result && balanced;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc > 2) return 64;
  if (argc == 2) {
    struct Case { const char* name; bool (*test)(); };
    const Case cases[] = {
      {"retained_payload", RetainedPayloadCase}, {"msg_peek", PeekCase},
      {"native_ancillary_discard", DiscardCase},
      {"carrier_close", CarrierCloseCase},
      {"plain_read_discard", PlainReadDiscardCase},
      {"full_control_manual_discard", FullControlDiscardCase},
      {"sender_child_death", ChildSenderCase},
      {"receiver_death_after_import", ChildReceiverCase},
      {"ctrunc_partial", CtruncCase},
    };
    for (const auto& item : cases)
      if (std::strcmp(argv[1], item.name) == 0)
        return RunCase(item.name, item.test, FdCount()) ? 0 : 1;
    return 64;
  }
  const int baseline = FdCount();
  if (!Check(baseline >= 0, "fd baseline")) return 2;
  bool ok = true;
  ok = RunCase("retained_payload", RetainedPayloadCase, baseline) && ok;
  ok = RunCase("msg_peek", PeekCase, baseline) && ok;
  ok = RunCase("native_ancillary_discard", DiscardCase, baseline) && ok;
  ok = RunCase("carrier_close", CarrierCloseCase, baseline) && ok;
  ok = RunCase("plain_read_discard", PlainReadDiscardCase, baseline) && ok;
  ok = RunCase("full_control_manual_discard", FullControlDiscardCase, baseline) && ok;
  ok = RunCase("sender_child_death", ChildSenderCase, baseline) && ok;
  ok = RunCase("receiver_death_after_import", ChildReceiverCase, baseline) && ok;
  ok = RunCase("ctrunc_partial", CtruncCase, baseline) && ok;
  std::printf("scm_pipe_guardian_fixture: %s\n", ok ? "PASS" : "FAIL");
  return ok ? 0 : 1;
}
