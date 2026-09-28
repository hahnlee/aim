#include <errno.h>
#include <netdb.h>
#include <netinet/in.h>
#include <stdint.h>
#include <sys/socket.h>
#include <unistd.h>

struct HttpFixtureResult {
  int32_t connected_family;
  int32_t send_calls;
  int32_t recv_calls;
  int32_t eintr_retries;
  int32_t last_android_errno;
  uint32_t response_length;
  char response[256];
};

static int retry_connect(int fd, const struct sockaddr* address,
                         socklen_t length, int32_t* retries) {
  for (;;) {
    if (connect(fd, address, length) == 0) return 0;
    if (errno != EINTR) return -1;
    ++*retries;
  }
}

__attribute__((visibility("default"))) int NetworkFixtureHttp(
    const char* node, const char* service, int family, int numeric_host,
    struct HttpFixtureResult* result) {
  if (result == (struct HttpFixtureResult*)0) return 100;
  result->connected_family = 0;
  result->send_calls = 0;
  result->recv_calls = 0;
  result->eintr_retries = 0;
  result->last_android_errno = 0;
  result->response_length = 0;
  for (size_t i = 0; i < sizeof(result->response); ++i) result->response[i] = 0;

  struct addrinfo hints = {0};
  hints.ai_flags = AI_NUMERICSERV | (numeric_host ? AI_NUMERICHOST : 0);
  hints.ai_family = family;
  hints.ai_socktype = SOCK_STREAM;
  hints.ai_protocol = IPPROTO_TCP;
  struct addrinfo* addresses = (struct addrinfo*)0;
  int gai = getaddrinfo(node, service, &hints, &addresses);
  if (gai != 0) return 110 + gai;

  int fd = -1;
  for (const struct addrinfo* current = addresses;
       current != (const struct addrinfo*)0; current = current->ai_next) {
    fd = socket(current->ai_family, current->ai_socktype,
                current->ai_protocol);
    if (fd < 0) continue;
    if (retry_connect(fd, current->ai_addr, current->ai_addrlen,
                      &result->eintr_retries) == 0) {
      result->connected_family = current->ai_family;
      result->last_android_errno = 0;
      break;
    }
    result->last_android_errno = errno;
    close(fd);
    fd = -1;
  }
  freeaddrinfo(addresses);
  if (fd < 0) return 120;

  static const char request[] =
      "GET /acceptance HTTP/1.0\r\nHost: localhost\r\n\r\n";
  size_t sent = 0;
  while (sent < sizeof(request) - 1) {
    size_t chunk = sizeof(request) - 1 - sent;
    if (chunk > 3) chunk = 3;
    ssize_t count = sent == 0
                        ? sendto(fd, request + sent, chunk, MSG_NOSIGNAL,
                                 (const struct sockaddr*)0, 0)
                        : send(fd, request + sent, chunk, MSG_NOSIGNAL);
    if (count < 0) {
      if (errno == EINTR) {
        ++result->eintr_retries;
        continue;
      }
      result->last_android_errno = errno;
      close(fd);
      return 121;
    }
    if (count == 0) {
      close(fd);
      return 122;
    }
    sent += (size_t)count;
    ++result->send_calls;
  }

  while (result->response_length < sizeof(result->response) - 1) {
    size_t capacity = sizeof(result->response) - 1 - result->response_length;
    if (capacity > 3) capacity = 3;
    ssize_t count = recv(fd, result->response + result->response_length,
                         capacity, 0);
    if (count < 0) {
      if (errno == EINTR) {
        ++result->eintr_retries;
        continue;
      }
      result->last_android_errno = errno;
      close(fd);
      return 123;
    }
    if (count == 0) break;
    result->response_length += (uint32_t)count;
    ++result->recv_calls;
  }
  result->response[result->response_length] = 0;
  if (close(fd) != 0) {
    result->last_android_errno = errno;
    return 124;
  }
  return 0;
}
