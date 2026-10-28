package com.android.server.adservices;
public final class SystemProcessLifecycle {
    private static final class AdServices extends com.android.server.SystemService {
        AdServices(android.content.Context context) { super(context); }
    }
    public static com.android.server.SystemService start(android.content.Context context) {
        var service = new AdServices(context);
        service.onStart();
        return service;
    }
}
