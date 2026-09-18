package dalvik.system;

/** Compile-only hidden Android API. Never packaged in runtime support DEX. */
public final class VMRuntime {
    public static native String getInstructionSet(String abi);
}
