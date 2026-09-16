package dev.darwinart.runtime.pm;

import android.os.Build;
import dalvik.system.VMRuntime;

/** ISA validation from PackageManagerServiceUtils, without its unrelated PMS dependencies. */
public final class DexInstructionSets {
    private DexInstructionSets() {}
    public static boolean checkISA(String isa) {
        for (String abi : Build.SUPPORTED_ABIS) {
            if (VMRuntime.getInstructionSet(abi).equals(isa)) return true;
        }
        return false;
    }
}
