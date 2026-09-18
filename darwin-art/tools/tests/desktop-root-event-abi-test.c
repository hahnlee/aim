#include "compat/window/desktop_root_surface.h"

_Static_assert(sizeof(DarwinArtDesktopRootEventKind) == 4, "fixed event kind ABI");
_Static_assert(sizeof(struct DarwinArtDesktopRootEvent) == 32, "event ABI");
_Static_assert(offsetof(struct DarwinArtDesktopRootEvent, incarnation) == 8, "identity ABI");
_Static_assert(offsetof(struct DarwinArtDesktopRootEvent, serial) == 16, "serial ABI");
_Static_assert(offsetof(struct DarwinArtDesktopRootEvent, key_window_snapshot) == 24,
               "snapshot ABI");
_Static_assert(sizeof(struct DarwinArtDesktopRootEventContext) == 24, "context ABI");
_Static_assert(sizeof(DarwinArtDesktopRootObserver) == 32, "observer ABI");
