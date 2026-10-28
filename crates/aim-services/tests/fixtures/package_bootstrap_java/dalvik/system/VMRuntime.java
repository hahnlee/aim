package dalvik.system;
public final class VMRuntime {
    public static boolean is64BitAbi(String abi) { return abi.equals("arm64-v8a"); }
}
