#pragma once

#include <cstdint>
#include <memory>
#include <utility>

namespace android {

using status_t = int32_t;
constexpr status_t OK = 0;
constexpr status_t DEAD_OBJECT = -32;

template <typename T>
class wp;

template <typename T>
class sp {
 public:
  sp() = default;
  sp(std::nullptr_t) {}
  explicit sp(std::shared_ptr<T> value) : value_(std::move(value)) {}
  template <typename U>
  sp(const sp<U>& other) : value_(other.value_) {}

  template <typename... Args>
  static sp<T> make(Args&&... args) {
    return sp<T>(std::make_shared<T>(std::forward<Args>(args)...));
  }

  T* get() const { return value_.get(); }
  T* operator->() const { return value_.get(); }
  explicit operator bool() const { return value_ != nullptr; }
  bool operator==(std::nullptr_t) const { return value_ == nullptr; }
  bool operator!=(std::nullptr_t) const { return value_ != nullptr; }

 private:
  template <typename>
  friend class sp;
  template <typename>
  friend class wp;
  std::shared_ptr<T> value_;
};

template <typename T>
class wp {
 public:
  wp() = default;
  wp(std::nullptr_t) {}
  template <typename U>
  wp(const sp<U>& strong) : value_(strong.value_) {}

 private:
  template <typename>
  friend class wp;
  std::weak_ptr<T> value_;
};

class BpBinder;

class IBinder : public std::enable_shared_from_this<IBinder> {
 public:
  class DeathRecipient {
   public:
    virtual ~DeathRecipient() = default;
    virtual void binderDied(const wp<IBinder>&) = 0;
  };

  virtual ~IBinder() = default;
  virtual BpBinder* remoteBinder() { return nullptr; }
  virtual bool isBinderAlive() const { return true; }
  virtual status_t linkToDeath(const sp<DeathRecipient>&) { return DEAD_OBJECT; }
  virtual status_t unlinkToDeath(const wp<DeathRecipient>&) { return DEAD_OBJECT; }
};

}  // namespace android
