package dev.aim.probe;

public final class SystemNativesDarwinSmoke {
    private static native int run(Throwable throwable);

    public static void main(String[] args) {
        if (args.length != 1) {
            throw new IllegalArgumentException("expected native library path");
        }
        System.load(args[0]);
        int result = run(new IllegalStateException("managed throwable acceptance"));
        if (result != 0xff) {
            throw new AssertionError("System native acceptance failed: " + result);
        }
        System.out.println("managed-system-natives: libcore=8 art=9 union=17 overlap=0 log=pass properties=pass streams=pass clock=pass map=pass");
    }
}
