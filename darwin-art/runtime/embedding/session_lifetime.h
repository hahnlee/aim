#pragma once

#include <cstdint>

#include "darwin_art/darwin_art.h"

namespace darwin_art_graphics {

struct GraphicsState;

// Admission is deliberately separate from the callback.  The owner validates
// the opaque handle while holding its short registry lock, then a move-only
// lease pins the allocation while Android-owned code runs unlocked.
enum class SessionAdmissionKind {
  kOwner,
  kWake,
};

class SessionLease final {
 public:
  // Definition is private to the lifetime implementation; the declaration
  // is public only so the implementation can name the pinned record.
  struct Record;

  SessionLease() = default;
  ~SessionLease();

  SessionLease(const SessionLease&) = delete;
  SessionLease& operator=(const SessionLease&) = delete;
  SessionLease(SessionLease&& other) noexcept;
  SessionLease& operator=(SessionLease&& other) noexcept;

  GraphicsState* state() const { return state_; }
  void* looper() const { return looper_; }
  explicit operator bool() const { return record_ != nullptr; }

 private:
  friend int32_t admit_session(darwin_art_graphics_session_t*,
                                SessionAdmissionKind, void*, bool,
                                SessionLease*);
  friend void set_session_looper(darwin_art_graphics_session_t*, void*);

  Record* record_ = nullptr;
  GraphicsState* state_ = nullptr;
  void* looper_ = nullptr;
};

darwin_art_graphics_session_t* create_session();
int32_t admit_session(darwin_art_graphics_session_t* session,
                      SessionAdmissionKind kind, void* caller_art_thread,
                      bool caller_art_native, SessionLease* lease);
// A membership/lifecycle-only preflight. Callers must complete this before
// looking up ART's current thread when handling an opaque raw handle.
int32_t preflight_session(darwin_art_graphics_session_t* session,
                          SessionAdmissionKind kind);
int32_t validate_session(darwin_art_graphics_session_t* session);
int32_t validate_state(GraphicsState* state);
void set_session_looper(darwin_art_graphics_session_t* session, void* looper);

int32_t bind_session_for_process_lifetime(void* context);
int32_t bind_session_art_thread_identity(void* thread_identity);
GraphicsState* state_for_context(void* context);

int32_t close_session(darwin_art_graphics_session_t* session,
                      void* caller_art_thread, bool caller_art_native);
// A bound session can only be destroyed after canonical ART shutdown has
// marked it finalized. Unbound closed sessions may be destroyed directly.
int32_t destroy_session(darwin_art_graphics_session_t* session);
// Read-only gate used before process shutdown commits its lifecycle phase.
bool bound_session_quiescent(GraphicsState* state);
// Marks the already closed/quiescent bound session safe for post-VM Drop.
int32_t finalize_bound_session(GraphicsState* state, void* caller_art_thread);

}  // namespace darwin_art_graphics
