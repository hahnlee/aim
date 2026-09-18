#include "session_lifetime.h"

#include <pthread.h>

#include <mutex>
#include <memory>
#include <new>
#include <unordered_set>

#include "graphics_state.h"

namespace darwin_art_graphics {

struct SessionLease::Record {
  GraphicsState state;
  pthread_t owner_thread{};
  void* owner_art_thread = nullptr;
  // The native Looper has process-wide lifetime. A wake operation borrows
  // this token while its lease prevents session close/finalization.
  void* owner_looper = nullptr;
  bool bound_to_process = false;
  bool closed = false;
  bool finalized = false;
  uint32_t operations = 0;
};

}  // namespace darwin_art_graphics

struct darwin_art_graphics_session_t {
  darwin_art_graphics::SessionLease::Record record;
};

namespace darwin_art_graphics {
namespace {

std::mutex g_session_mutex;
std::unordered_set<darwin_art_graphics_session_t*> g_sessions;
darwin_art_graphics_session_t* g_process_session = nullptr;

darwin_art_graphics_session_t* find_session_locked(
    darwin_art_graphics_session_t* session) {
  // Comparing an opaque pointer against the registry is safe even when the
  // caller retained a stale value. Do not dereference it until membership is
  // established by this lookup.
  if (session == nullptr || g_sessions.find(session) == g_sessions.end())
    return nullptr;
  return session;
}

bool owner_thread_locked(const SessionLease::Record& record) {
  return pthread_equal(record.owner_thread, pthread_self()) != 0;
}

int32_t admit_owner_locked(darwin_art_graphics_session_t* session,
                           void* caller_art_thread, bool caller_art_native,
                           SessionLease::Record** admitted) {
  if (session->record.closed || session->record.finalized)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_CLOSED;
  if (!session->record.bound_to_process ||
      session->record.owner_art_thread == nullptr)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY;
  if (!owner_thread_locked(session->record) ||
      caller_art_thread != session->record.owner_art_thread ||
      !caller_art_native)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD;

  ++session->record.operations;
  *admitted = &session->record;
  return 0;
}

int32_t admit_wake_locked(darwin_art_graphics_session_t* session,
                          SessionLease::Record** admitted) {
  if (session->record.closed || session->record.finalized)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_CLOSED;
  if (!session->record.bound_to_process ||
      session->record.owner_looper == nullptr)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY;

  // No ART::Thread lookup is permitted on this cross-thread path. The lease
  // pins the native allocation until the wake callback has consumed the token.
  ++session->record.operations;
  *admitted = &session->record;
  return 0;
}

void release_record(SessionLease::Record* record) {
  if (record == nullptr) return;
  std::lock_guard<std::mutex> lock(g_session_mutex);
  // A lease is the only thing that can keep a record alive after admission;
  // destroy_session therefore cannot remove its containing allocation while
  // this decrement is pending.
  if (record->operations != 0) --record->operations;
}

}  // namespace

SessionLease::~SessionLease() {
  release_record(record_);
  record_ = nullptr;
  state_ = nullptr;
  looper_ = nullptr;
}

SessionLease::SessionLease(SessionLease&& other) noexcept
    : record_(other.record_), state_(other.state_), looper_(other.looper_) {
  other.record_ = nullptr;
  other.state_ = nullptr;
  other.looper_ = nullptr;
}

SessionLease& SessionLease::operator=(SessionLease&& other) noexcept {
  if (this == &other) return *this;
  release_record(record_);
  record_ = other.record_;
  state_ = other.state_;
  looper_ = other.looper_;
  other.record_ = nullptr;
  other.state_ = nullptr;
  other.looper_ = nullptr;
  return *this;
}

darwin_art_graphics_session_t* create_session() {
  std::lock_guard<std::mutex> lock(g_session_mutex);
  std::unique_ptr<darwin_art_graphics_session_t> session(
      new (std::nothrow) darwin_art_graphics_session_t);
  if (session == nullptr) return nullptr;
  session->record.owner_thread = pthread_self();
  try {
    g_sessions.insert(session.get());
  } catch (const std::bad_alloc&) {
    // Publication did not succeed: RAII frees the unpublished allocation and
    // the C ABI reports allocation failure without leaking an exception.
    return nullptr;
  }
  return session.release();
}

int32_t admit_session(darwin_art_graphics_session_t* session,
                      SessionAdmissionKind kind, void* caller_art_thread,
                      bool caller_art_native, SessionLease* lease) {
  if (lease == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  // Reusing a lease would otherwise leak its previous admission count. The
  // move-only type is intentionally reset by assignment/destruction instead.
  if (*lease) return DARWIN_ART_STATUS_GRAPHICS_SESSION_ALREADY_ACTIVE;

  std::lock_guard<std::mutex> lock(g_session_mutex);
  session = find_session_locked(session);
  if (session == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  SessionLease::Record* admitted = nullptr;
  const int32_t status =
      kind == SessionAdmissionKind::kWake
          ? admit_wake_locked(session, &admitted)
          : admit_owner_locked(session, caller_art_thread, caller_art_native,
                               &admitted);
  if (status != 0) return status;
  lease->record_ = admitted;
  lease->state_ = kind == SessionAdmissionKind::kWake
                      ? nullptr
                      : &session->record.state;
  lease->looper_ = session->record.owner_looper;
  return 0;
}

int32_t preflight_session(darwin_art_graphics_session_t* session,
                          SessionAdmissionKind kind) {
  std::lock_guard<std::mutex> lock(g_session_mutex);
  session = find_session_locked(session);
  if (session == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  if (session->record.closed || session->record.finalized)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_CLOSED;
  if (kind == SessionAdmissionKind::kOwner &&
      !owner_thread_locked(session->record))
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD;
  if (!session->record.bound_to_process)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY;
  if (kind == SessionAdmissionKind::kWake &&
      session->record.owner_looper == nullptr)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY;
  if (kind == SessionAdmissionKind::kOwner &&
      session->record.owner_art_thread == nullptr)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY;
  return 0;
}

int32_t validate_session(darwin_art_graphics_session_t* session) {
  std::lock_guard<std::mutex> lock(g_session_mutex);
  session = find_session_locked(session);
  if (session == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  if (session->record.closed || session->record.finalized)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_CLOSED;
  if (!owner_thread_locked(session->record))
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD;
  return 0;
}

int32_t validate_state(GraphicsState* state) {
  if (state == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  std::lock_guard<std::mutex> lock(g_session_mutex);
  for (auto* session : g_sessions) {
    if (&session->record.state != state) continue;
    return 0;
  }
  return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
}

void set_session_looper(darwin_art_graphics_session_t* session, void* looper) {
  if (session == nullptr || looper == nullptr) return;
  std::lock_guard<std::mutex> lock(g_session_mutex);
  if (find_session_locked(session) == nullptr || session->record.closed ||
      session->record.finalized)
    return;
  session->record.owner_looper = looper;
}

int32_t bind_session_for_process_lifetime(void* context) {
  if (context == nullptr) return 0;
  std::lock_guard<std::mutex> lock(g_session_mutex);
  auto* session = find_session_locked(
      static_cast<darwin_art_graphics_session_t*>(context));
  if (session == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  if (!owner_thread_locked(session->record))
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD;
  if (session->record.closed || session->record.finalized)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_CLOSED;
  if (session->record.bound_to_process || g_process_session != nullptr)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_ALREADY_ACTIVE;
  session->record.bound_to_process = true;
  g_process_session = session;
  return 0;
}

int32_t bind_session_art_thread_identity(void* thread_identity) {
  if (thread_identity == nullptr)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  std::lock_guard<std::mutex> lock(g_session_mutex);
  auto* session = find_session_locked(g_process_session);
  if (session == nullptr || !session->record.bound_to_process ||
      session->record.closed || session->record.finalized)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  if (!owner_thread_locked(session->record))
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD;
  session->record.owner_art_thread = thread_identity;
  return 0;
}

int32_t close_session(darwin_art_graphics_session_t* session,
                      void* caller_art_thread, bool caller_art_native) {
  std::lock_guard<std::mutex> lock(g_session_mutex);
  session = find_session_locked(session);
  if (session == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  if (session->record.closed)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_CLOSED;
  if (session->record.finalized)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_CLOSED;
  if (!owner_thread_locked(session->record) ||
      (session->record.owner_art_thread != nullptr &&
       caller_art_thread != session->record.owner_art_thread))
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD;
  if (session->record.owner_art_thread != nullptr && !caller_art_native)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD;
  // Status 74 is the existing public retryable-not-ready status. Never wait:
  // this also makes a reentrant close from an admitted callback deadlock-free.
  if (session->record.operations != 0)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY;
  session->record.closed = true;
  return 0;
}

GraphicsState* state_for_context(void* context) {
  std::lock_guard<std::mutex> lock(g_session_mutex);
  auto* session = find_session_locked(
      static_cast<darwin_art_graphics_session_t*>(context));
  return session == nullptr ? nullptr : &session->record.state;
}

bool bound_session_quiescent(GraphicsState* state) {
  std::lock_guard<std::mutex> lock(g_session_mutex);
  for (auto* session : g_sessions) {
    if (&session->record.state != state) continue;
    return session->record.bound_to_process && session->record.closed &&
           !session->record.finalized && session->record.operations == 0;
  }
  return false;
}

int32_t finalize_bound_session(GraphicsState* state, void* caller_art_thread) {
  if (state == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  std::lock_guard<std::mutex> lock(g_session_mutex);
  for (auto* session : g_sessions) {
    if (&session->record.state != state) continue;
    if (!session->record.bound_to_process || !session->record.closed ||
        session->record.finalized)
      return DARWIN_ART_STATUS_GRAPHICS_SESSION_CLOSED;
    if (session->record.operations != 0)
      return DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY;
    if (!owner_thread_locked(session->record) ||
        session->record.owner_art_thread == nullptr ||
        caller_art_thread != session->record.owner_art_thread)
      return DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD;
    session->record.finalized = true;
    return 0;
  }
  return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
}

int32_t destroy_session(darwin_art_graphics_session_t* session) {
  std::lock_guard<std::mutex> lock(g_session_mutex);
  session = find_session_locked(session);
  if (session == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  if (session->record.operations != 0)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY;
  if (!session->record.closed)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_CLOSED;
  if (session->record.bound_to_process && !session->record.finalized)
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY;

  // Finalized records are safe to erase from any thread after ART has gone
  // away. No ART::Thread lookup is made on this path.
  if (!session->record.finalized && !owner_thread_locked(session->record))
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD;
  if (g_process_session == session) g_process_session = nullptr;
  g_sessions.erase(session);
  delete session;
  return 0;
}

}  // namespace darwin_art_graphics
