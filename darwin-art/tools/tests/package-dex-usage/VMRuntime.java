package dalvik.system;
public final class VMRuntime {
    public static String getInstructionSet(String abi) {
        if (!"arm64-v8a".equals(abi)) throw new IllegalArgumentException(abi);
        return "arm64";
    }
}
