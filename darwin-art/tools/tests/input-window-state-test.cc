#include "runtime/framework/input/input_window_state.h"
#include <cassert>
using namespace darwin_art::input;
int main() {
  InputWindowState state;
  assert(!state.Eligible());
  assert(state.UpdateReceiverFrame({0, 0, 100, 100}));
  assert(!state.Eligible());
  auto transition = state.PublishWmsFrame({10, 20, 110, 120}, true);
  assert(!transition.was_eligible && transition.is_eligible);
  assert(!state.UpdateReceiverFrame({0, 0, 200, 200}));
  assert(state.Frame().left == 10 && state.Eligible());
  transition = state.PublishWmsFrame({10, 20, 110, 120}, false);
  assert(transition.Revoked());
  assert(!state.UpdateReceiverFrame({0, 0, 200, 200}) && !state.Eligible());
  transition = state.PublishWmsFrame({0, 0, 100, 100}, true);
  assert(transition.is_eligible);
  transition = state.PublishWmsFrame({0, 0, 0, 100}, true);
  assert(transition.Revoked());
  assert(!state.UpdateReceiverFrame({0, 0, 100, 100}) && !state.Eligible());
}
