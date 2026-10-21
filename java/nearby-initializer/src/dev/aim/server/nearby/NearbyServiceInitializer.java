package dev.aim.server.nearby;

import android.content.Context;

import com.android.server.SystemService;
import com.android.server.nearby.NearbyService;

/**
 * ConnectivityServiceInitializer's part for Nearby (android-16.0.0_r1): the
 * original NearbyService of the Connectivity module's standalone system
 * server jar, created in the initializer's constructor, published as
 * `nearby` in its `onStart` and given its boot phases. The app host serves
 * the initializer's other services (connectivity, ipsec, nsd, ethernet)
 * natively; the system process starts this in a class loader below the
 * jar's, as SystemServiceManager starts the initializer from it
 * (`startServiceFromJar`).
 */
public final class NearbyServiceInitializer extends SystemService {
    /** `ConstantsShim.NEARBY_SERVICE`. */
    private static final String NEARBY_SERVICE = "nearby";

    private final NearbyService mNearbyService;

    public NearbyServiceInitializer(Context context) {
        super(context);
        mNearbyService = new NearbyService(context);
    }

    @Override
    public void onStart() {
        publishBinderService(NEARBY_SERVICE, mNearbyService, /* allowIsolated= */ false);
    }

    @Override
    public void onBootPhase(int phase) {
        mNearbyService.onBootPhase(phase);
    }
}
