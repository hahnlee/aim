public final class SystemProcessEntryTest {
    private static native int run(int mode);
    public static void main(String[] args) {
        System.load(args[0]);
        if (run(0) != 23) throw new AssertionError("endpoint result lost");
        if (run(1) != 70) throw new AssertionError("missing capability accepted");
        try {
            run(2);
            throw new AssertionError("initialization exception swallowed");
        } catch (IllegalStateException expected) {
            if (!"initialization failed".equals(expected.getMessage())) throw expected;
        }
        if (run(3) != 70) throw new AssertionError("compositor failure accepted");
        System.out.println("system-process-entry: PASS (isolated JNI, no Probe classes)");
    }
}
