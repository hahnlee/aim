#include "compat/window/desktop_root_surface.h"

_Static_assert(sizeof(AimDesktopRootEventKind) == 4, "fixed event kind ABI");
_Static_assert(sizeof(struct AimDesktopRootEvent) == 32, "event ABI");
_Static_assert(offsetof(struct AimDesktopRootEvent, incarnation) == 8, "identity ABI");
_Static_assert(offsetof(struct AimDesktopRootEvent, serial) == 16, "serial ABI");
_Static_assert(offsetof(struct AimDesktopRootEvent, key_window_snapshot) == 24,
               "snapshot ABI");
_Static_assert(sizeof(struct AimDesktopRootEventContext) == 24, "context ABI");
_Static_assert(sizeof(AimDesktopRootObserver) == 32, "observer ABI");
