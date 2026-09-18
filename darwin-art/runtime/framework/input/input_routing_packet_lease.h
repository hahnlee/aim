#pragma once

// Public packet-lease surface is declared with the routing owner types.  This
// narrow include lets receiver-consumption owners depend on the lease without
// reaching into InputRoutingStateData.
#include "input_routing.h"
