package dev.aim.probe;

public final class NetworkRuntimeFixture {
    private NetworkRuntimeFixture() {}

    public static native int nativeLoopbackHttp(int port);
}
