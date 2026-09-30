package dev.aim.server;

import dev.aim.server.IBridge;

/**
 * The native service host (crates/aim-services), registered with
 * servicemanager as `aim.service_host`.
 */
oneway interface IServiceHost {
    /** system_server's bridge, once its system services are ready. */
    void attachBridge(IBridge bridge);
}
