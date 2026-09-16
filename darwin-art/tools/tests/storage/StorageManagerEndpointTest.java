package dev.darwinart.runtime.storage;

/** Focused contract test for the system-owned primary external volume. */
public final class StorageManagerEndpointTest {
    private static void check(boolean value) {
        if (!value) throw new AssertionError();
    }

    public static void main(String[] args) {
        check("/storage/emulated/0".equals(StorageManagerEndpoint.requirePrimaryPath(
                "/storage/emulated/0")));
        try {
            StorageManagerEndpoint.requirePrimaryPath("relative");
            throw new AssertionError("accepted a relative storage path");
        } catch (IllegalArgumentException expected) {}
        System.out.println("StorageManagerEndpoint primary volume path PASS");
    }
}
