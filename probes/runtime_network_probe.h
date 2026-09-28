#pragma once

#include <atomic>
#include <mutex>
#include <thread>

class BoundedLoopbackHttpServer final {
 public:
  ~BoundedLoopbackHttpServer();

  bool Start();
  int port() const;
  bool Stop();

 private:
  void Serve();

  int listener_ = -1;
  mutable std::mutex listener_mutex_;
  int port_ = 0;
  std::thread thread_;
  std::atomic<bool> cancelled_{false};
  std::atomic<bool> success_{false};
  bool joined_ = false;
};
