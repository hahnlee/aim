#pragma once

namespace art { class Thread; }

namespace darwin_art::runtime_art {
// Conservative snapshot only. ART retains its real DestroyJavaVM wait/birth
// synchronization. This never interrupts or shuts down app-owned threads.
bool IsVmReadyForShutdown(art::Thread* owner);
}
