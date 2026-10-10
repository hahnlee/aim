package dev.aim.server;
import android.os.ParcelFileDescriptor;
/** Read-only original parsing/flags over actual shell-transferred APK data. */
interface IPackageShellInstallPolicy {
    long calculateInstalledSize(in ParcelFileDescriptor file, String path, String abi, int callingUid, int callingPid);
    void validateAbi(String abi, int callingUid, int callingPid);
    void validateCompilerFilter(String filter, int callingUid, int callingPid);
    boolean isDependencyInstallerEnabled(int callingUid, int callingPid);
}
