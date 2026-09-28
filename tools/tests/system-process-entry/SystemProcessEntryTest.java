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
        if (run(4) != 70) throw new AssertionError("Binder startup failure accepted");
        if (run(5) != 70) throw new AssertionError("missing system Context accepted");
        try {
            run(6);
            throw new AssertionError("compat initialization exception swallowed");
        } catch (IllegalStateException expected) {
            if (!"compat catalog failed".equals(expected.getMessage())) throw expected;
        }
        System.out.println("system-process-entry: PASS (isolated JNI, no Probe classes)");
    }
}
