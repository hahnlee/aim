// Compile-only pinned image API; checked against original classes.
package dalvik.system;
public final class VMRuntime {
    public static int getSdkVersion() { throw new RuntimeException("stub"); }
    public static boolean is64BitAbi(String abi) {throw new RuntimeException("stub");}
    public static VMRuntime getRuntime(){throw new RuntimeException("stub");}
    public boolean is64Bit(){throw new RuntimeException("stub");}
 public static String getInstructionSet(String abi){throw new RuntimeException("stub");}
}
