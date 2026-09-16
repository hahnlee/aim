package com.android.server.pm;
public final class PackageManagerServiceUtils {
    public static boolean checkISA(String isa) {
        for (String abi : android.os.Build.SUPPORTED_ABIS) {
            if (dalvik.system.VMRuntime.getInstructionSet(abi).equals(isa)) return true;
        }
        return false;
    }
}
