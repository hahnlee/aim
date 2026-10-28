package com.android.internal.os;
public final class BackgroundThread {
    private static final android.os.Handler handler = new android.os.Handler();
    public static android.os.Handler getHandler() { return handler; }
}
