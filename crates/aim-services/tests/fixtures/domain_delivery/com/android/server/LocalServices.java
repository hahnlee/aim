package com.android.server;
public final class LocalServices {
    public static final DeviceIdleInternal idle=new DeviceIdleInternal();
    public static <T> T getService(Class<T> type){return type.cast(idle);}
}
