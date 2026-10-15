package dev.aim.server;
import android.os.Binder;
import android.os.Process;
import android.os.ParcelFileDescriptor;
import android.os.Build;
import android.content.pm.parsing.ApkLiteParseUtils;
import android.content.pm.parsing.PackageLite;
import android.content.pm.parsing.result.ParseTypeImpl;
import com.android.internal.content.InstallLocationUtils;
import com.android.server.art.model.DexoptParams;
import com.android.server.art.ReasonMapping;
/** Independent original read-only install policy. Native retains actual caller. */
public final class PackageShellInstallPolicy extends IPackageShellInstallPolicy.Stub {
    private static void enforce(int uid,int pid){
        if(Binder.getCallingUid()!=Process.SYSTEM_UID || (uid!=0&&uid!=Process.SHELL_UID)||pid<0)
            throw new SecurityException("Invalid native shell policy caller");
    }
    @Override public long calculateInstalledSize(ParcelFileDescriptor file,String path,String abi,int uid,int pid){
        enforce(uid,pid);java.util.Objects.requireNonNull(file);java.util.Objects.requireNonNull(path);
        var result=ApkLiteParseUtils.parseApkLite(ParseTypeImpl.forDefaultParsing().reset(),file.getFileDescriptor(),path,0);
        if(result.isError())throw new IllegalArgumentException("Error: Failed to parse APK file: "+path+": "+result.getErrorMessage(),result.getException());
        var apk=result.getResult();
        var lite=new PackageLite(null,apk.getPath(),apk,null,null,null,null,null,null,apk.getTargetSdkVersion(),null,null);
        try{return InstallLocationUtils.calculateInstalledSize(lite,abi,file.getFileDescriptor());}
        catch(java.io.IOException error){throw new IllegalArgumentException("Error: Failed to parse APK file: "+path,error);}
    }
    @Override public void validateAbi(String abi,int uid,int pid){
        enforce(uid,pid);if(abi==null||abi.isEmpty())throw new IllegalArgumentException("Missing ABI argument");
        if(abi.equals("-"))return;
        for(String supported:Build.SUPPORTED_ABIS)if(supported.equals(abi))return;
        throw new IllegalArgumentException("ABI "+abi+" not supported on this device");
    }
    @Override public void validateCompilerFilter(String filter,int uid,int pid){
        enforce(uid,pid);new DexoptParams.Builder(ReasonMapping.REASON_INSTALL).setCompilerFilter(filter).build();
    }
    @Override public boolean isDependencyInstallerEnabled(int uid,int pid){enforce(uid,pid);return com.android.internal.hidden_from_bootclasspath.android.content.pm.Flags.sdkDependencyInstaller();}
}
