package dev.darwinart.runtime.pm.art;

import android.os.ServiceSpecificException;
import com.android.server.art.ArtifactsPath;
import com.android.server.art.CopyAndRewriteProfileResult;
import com.android.server.art.DexMetadataPath;
import com.android.server.art.GetDexoptNeededResult;
import com.android.server.art.GetDexoptStatusResult;
import com.android.server.art.IArtd;
import com.android.server.art.IArtdCancellationSignal;
import com.android.server.art.IArtdNotification;
import com.android.server.art.MergeProfileOptions;
import com.android.server.art.OutputProfile;
import com.android.server.art.OutputSecureDexMetadataCompanion;
import com.android.server.art.ProfilePath;
import com.android.server.art.RuntimeArtifactsPath;
import com.android.server.art.SecureDexMetadataWithCompanionPaths;
import com.android.server.art.VdexPath;
import dalvik.system.DexFile;
import java.io.File;
import java.io.FileNotFoundException;
import java.util.List;

/**
 * The {@code artd} service for this runtime (ADR 0009).
 *
 * <p>ART Service keeps its file bookkeeping in artd: profiles, dexopt
 * artifacts and their visibility. Those operations are ported from artd
 * (artd.cc, path_utils.cc) over this process's Android filesystem. This
 * runtime's ART executes on the host with its own boot image, so dexopt
 * (dex2oat) is off ({@code dalvik.vm.disable-art-service-dexopt}) and every
 * operation that would compile, merge or rewrite profiles fails with
 * EOPNOTSUPP instead of pretending to succeed. Only system-server callers
 * reach it; there is no artd transaction handling across processes.</p>
 */
public final class DarwinArtd extends android.os.Binder implements IArtd {
    public static final String SERVICE_NAME = "artd";
    // The ART module's IArtd.Stub is reduced to asInterface (artd is native
    // on Android), which returns this object for in-process callers.
    private static final String DESCRIPTOR = "com.android.server.art.IArtd";

    public DarwinArtd() {
        attachInterface(this, DESCRIPTOR);
    }

    @Override
    public android.os.IBinder asBinder() {
        return this;
    }
    // Android errno values reported in ServiceSpecificException.
    private static final int EINVAL = 22;
    private static final int EOPNOTSUPP = 95;

    // ProfilePath and ProfilePath.WritableProfilePath union tags (AIDL
    // declaration order).
    private static final int PRIMARY_REF = 0;
    private static final int PREBUILT = 1;
    private static final int PRIMARY_CUR = 2;
    private static final int SECONDARY_REF = 3;
    private static final int SECONDARY_CUR = 4;
    private static final int TMP = 5;
    private static final int DEX_METADATA = 6;
    private static final int FOR_PRIMARY = 0;
    private static final int FOR_SECONDARY = 1;

    // FileVisibility.
    private static final int NOT_FOUND = 0;
    private static final int OTHER_READABLE = 1;
    private static final int NOT_OTHER_READABLE = 2;
    // ArtifactsLocation.NONE_OR_ERROR.
    private static final int LOCATION_NONE_OR_ERROR = 0;

    private static final String ANDROID_DATA = "/data";
    private static final String PROFILE_EXT = ".prof";
    private static final String PRE_REBOOT_SUFFIX = ".staged";

    private static ServiceSpecificException unsupported(String operation) {
        return new ServiceSpecificException(EOPNOTSUPP,
                "artd." + operation + " is not provided by this runtime");
    }

    private static ServiceSpecificException invalid(String message) {
        return new ServiceSpecificException(EINVAL, message);
    }

    // path_utils.cc

    private static void validatePathElement(String element, String name) {
        if (element == null || element.isEmpty() || element.equals(".")
                || element.equals("..") || element.contains("/") || element.contains("\0")) {
            throw invalid(name + " is not a valid path element: " + element);
        }
    }

    private static void checkDexPath(String dexPath) {
        if (dexPath == null || !dexPath.startsWith("/") || dexPath.contains("\0")
                || !new File(dexPath).toPath().normalize().toString().equals(dexPath)) {
            throw invalid("Path '" + dexPath + "' is not an absolute normalized path");
        }
    }

    private static String replaceExtension(String path, String extension) {
        int slash = path.lastIndexOf('/');
        int dot = path.lastIndexOf('.');
        return (dot > slash ? path.substring(0, dot) : path) + extension;
    }

    private static String primaryRefProfile(ProfilePath.PrimaryRefProfilePath path) {
        validatePathElement(path.packageName, "packageName");
        validatePathElement(path.profileName, "profileName");
        return ANDROID_DATA + "/misc/profiles/ref/" + path.packageName + "/" + path.profileName
                + PROFILE_EXT + (path.isPreReboot ? PRE_REBOOT_SUFFIX : "");
    }

    private static String secondaryRefProfile(ProfilePath.SecondaryRefProfilePath path) {
        checkDexPath(path.dexPath);
        File dex = new File(path.dexPath);
        return dex.getParent() + "/oat/" + dex.getName() + PROFILE_EXT
                + (path.isPreReboot ? PRE_REBOOT_SUFFIX : "");
    }

    private static String writableProfile(ProfilePath.WritableProfilePath path) {
        switch (path.getTag()) {
            case FOR_PRIMARY: return primaryRefProfile(path.getForPrimary());
            case FOR_SECONDARY: return secondaryRefProfile(path.getForSecondary());
            default: throw invalid("Unexpected writable profile path type " + path.getTag());
        }
    }

    private static String dexMetadata(DexMetadataPath path) {
        checkDexPath(path.dexPath);
        return replaceExtension(path.dexPath, ".dm");
    }

    /** BuildProfileOrDmPath. */
    static String profileOrDmPath(ProfilePath profile) {
        switch (profile.getTag()) {
            case PRIMARY_REF:
                return primaryRefProfile(profile.getPrimaryRefProfilePath());
            case PREBUILT: {
                String dexPath = profile.getPrebuiltProfilePath().dexPath;
                checkDexPath(dexPath);
                return dexPath + PROFILE_EXT;
            }
            case PRIMARY_CUR: {
                ProfilePath.PrimaryCurProfilePath path = profile.getPrimaryCurProfilePath();
                validatePathElement(path.packageName, "packageName");
                validatePathElement(path.profileName, "profileName");
                return ANDROID_DATA + "/misc/profiles/cur/" + path.userId + "/"
                        + path.packageName + "/" + path.profileName + PROFILE_EXT;
            }
            case SECONDARY_REF:
                return secondaryRefProfile(profile.getSecondaryRefProfilePath());
            case SECONDARY_CUR: {
                String dexPath = profile.getSecondaryCurProfilePath().dexPath;
                checkDexPath(dexPath);
                File dex = new File(dexPath);
                return dex.getParent() + "/oat/" + dex.getName() + ".cur" + PROFILE_EXT;
            }
            case TMP: {
                ProfilePath.TmpProfilePath path = profile.getTmpProfilePath();
                validatePathElement(path.id, "id");
                // NewFile::BuildTempPath
                return writableProfile(path.finalPath) + "." + path.id + ".tmp";
            }
            case DEX_METADATA:
                return dexMetadata(profile.getDexMetadataPath());
            default:
                throw invalid("Unexpected profile path type " + profile.getTag());
        }
    }

    /** BuildArtifactsPath: {oat, vdex, art}. */
    static String[] artifactsPaths(ArtifactsPath artifacts) {
        checkDexPath(artifacts.dexPath);
        if (!"arm64".equals(artifacts.isa)) throw invalid("Unsupported ISA " + artifacts.isa);
        File dex = new File(artifacts.dexPath);
        String oat;
        if (artifacts.isInDalvikCache) {
            // OatFileAssistant::DexLocationToOatFilename: dalvik-cache/<isa>/<location>@classes.dex
            oat = ANDROID_DATA + "/dalvik-cache/" + artifacts.isa + "/"
                    + artifacts.dexPath.substring(1).replace('/', '@') + "@classes.dex";
        } else {
            // OatFileAssistant::DexLocationToOdexFilename: <dir>/oat/<isa>/<name>.odex
            oat = dex.getParent() + "/oat/" + artifacts.isa + "/"
                    + replaceExtension(dex.getName(), ".odex");
        }
        String suffix = artifacts.isPreReboot ? PRE_REBOOT_SUFFIX : "";
        return new String[] {oat + suffix, replaceExtension(oat, ".vdex") + suffix,
                replaceExtension(oat, ".art") + suffix};
    }

    private static int visibility(String path) {
        File file = new File(path);
        if (!file.exists()) return NOT_FOUND;
        try {
            int mode = android.system.Os.stat(path).st_mode;
            return (mode & 0004) != 0 ? OTHER_READABLE : NOT_OTHER_READABLE;
        } catch (android.system.ErrnoException error) {
            throw new ServiceSpecificException(error.errno, "Failed to stat " + path);
        }
    }

    private static long size(String path) {
        File file = new File(path);
        return file.isFile() ? file.length() : 0;
    }

    private static long sizeAndDelete(String path) {
        long size = size(path);
        File file = new File(path);
        if (file.exists() && !file.delete()) return 0;
        return size;
    }

    @Override
    public void deleteProfile(ProfilePath profile) {
        new File(profileOrDmPath(profile)).delete();
    }

    @Override
    public int getProfileVisibility(ProfilePath profile) {
        return visibility(profileOrDmPath(profile));
    }

    @Override
    public long getProfileSize(ProfilePath profile) {
        return size(profileOrDmPath(profile));
    }

    @Override
    public int getArtifactsVisibility(ArtifactsPath artifacts) {
        return visibility(artifactsPaths(artifacts)[0]);
    }

    @Override
    public long getArtifactsSize(ArtifactsPath artifacts) {
        long total = 0;
        for (String path : artifactsPaths(artifacts)) total += size(path);
        return total;
    }

    @Override
    public long deleteArtifacts(ArtifactsPath artifacts) {
        long total = 0;
        for (String path : artifactsPaths(artifacts)) total += sizeAndDelete(path);
        return total;
    }

    @Override
    public long getVdexFileSize(VdexPath vdex) {
        return size(artifactsPaths(vdex.getArtifactsPath())[1]);
    }

    @Override
    public int getDexFileVisibility(String dexFile) {
        checkDexPath(dexFile);
        return visibility(dexFile);
    }

    @Override
    public boolean isInDalvikCache(String dexFile) {
        checkDexPath(dexFile);
        // The dex file's mount is read-only: the immutable image (system,
        // product, APEX). Installed and app-private code is on /data.
        return !dexFile.startsWith(ANDROID_DATA + "/");
    }

    /**
     * The runtime's own OatFileAssistant status for the dex file, as artd
     * reports it. The location of the artifacts is not part of that answer.
     */
    @Override
    public GetDexoptStatusResult getDexoptStatus(String dexFile, String instructionSet,
            String classLoaderContext) {
        GetDexoptStatusResult result = new GetDexoptStatusResult();
        try {
            DexFile.OptimizationInfo info =
                    DexFile.getDexFileOptimizationInfo(dexFile, instructionSet);
            result.compilerFilter = info.getStatus();
            result.compilationReason = info.getReason();
        } catch (FileNotFoundException error) {
            throw new ServiceSpecificException(2 /* ENOENT */, error.getMessage());
        }
        result.locationDebugString = "unknown";
        result.artifactsLocation = LOCATION_NONE_OR_ERROR;
        return result;
    }

    @Override
    public long getRuntimeArtifactsSize(RuntimeArtifactsPath path) {
        throw unsupported("getRuntimeArtifactsSize");
    }

    @Override
    public long deleteRuntimeArtifacts(RuntimeArtifactsPath path) {
        throw unsupported("deleteRuntimeArtifacts");
    }

    @Override
    public boolean isProfileUsable(ProfilePath profile, String dexFile) {
        throw unsupported("isProfileUsable");
    }

    @Override
    public CopyAndRewriteProfileResult copyAndRewriteProfile(ProfilePath src, OutputProfile dst,
            String dexFile) {
        throw unsupported("copyAndRewriteProfile");
    }

    @Override
    public CopyAndRewriteProfileResult copyAndRewriteEmbeddedProfile(OutputProfile dst,
            String dexFile) {
        throw unsupported("copyAndRewriteEmbeddedProfile");
    }

    @Override
    public void commitTmpProfile(ProfilePath.TmpProfilePath profile) {
        throw unsupported("commitTmpProfile");
    }

    @Override
    @SuppressWarnings("rawtypes")
    public boolean mergeProfiles(List profiles, ProfilePath referenceProfile,
            OutputProfile output, List dexFiles, MergeProfileOptions options) {
        throw unsupported("mergeProfiles");
    }

    @Override
    public GetDexoptNeededResult getDexoptNeeded(String dexFile, String instructionSet,
            String classLoaderContext, String compilerFilter, int dexoptTrigger) {
        throw unsupported("getDexoptNeeded");
    }

    @Override
    public com.android.server.art.ArtdDexoptResult dexopt(
            com.android.server.art.OutputArtifacts outputArtifacts, String dexFile,
            String instructionSet, String classLoaderContext, String compilerFilter,
            ProfilePath profile, VdexPath inputVdex, DexMetadataPath dmFile, int priorityClass,
            com.android.server.art.DexoptOptions dexoptOptions,
            IArtdCancellationSignal cancellationSignal) {
        throw unsupported("dexopt");
    }

    @Override
    public IArtdCancellationSignal createCancellationSignal() {
        throw unsupported("createCancellationSignal");
    }

    @Override
    @SuppressWarnings("rawtypes")
    public long cleanup(List profilesToKeep, List artifactsToKeep, List vdexFilesToKeep,
            List sdmSdcFilesToKeep, List runtimeArtifactsToKeep,
            boolean keepPreRebootStagedFiles) {
        throw unsupported("cleanup");
    }

    @Override
    public void cleanUpPreRebootStagedFiles() {
        throw unsupported("cleanUpPreRebootStagedFiles");
    }

    @Override
    @SuppressWarnings("rawtypes")
    public boolean commitPreRebootStagedFiles(List artifacts, List profiles) {
        throw unsupported("commitPreRebootStagedFiles");
    }

    @Override
    public boolean checkPreRebootSystemRequirements(String chrootDir) {
        throw unsupported("checkPreRebootSystemRequirements");
    }

    @Override
    public boolean preRebootInit(IArtdCancellationSignal cancellationSignal) {
        throw unsupported("preRebootInit");
    }

    @Override
    public String validateDexPath(String dexFile) {
        throw unsupported("validateDexPath");
    }

    @Override
    public String validateClassLoaderContext(String dexFile, String classLoaderContext) {
        throw unsupported("validateClassLoaderContext");
    }

    @Override
    public IArtdNotification initProfileSaveNotification(
            ProfilePath.PrimaryCurProfilePath profilePath, int pid) {
        throw unsupported("initProfileSaveNotification");
    }

    @Override
    public long deleteSdmSdcFiles(SecureDexMetadataWithCompanionPaths paths) {
        throw unsupported("deleteSdmSdcFiles");
    }

    @Override
    public long getSdmFileSize(SecureDexMetadataWithCompanionPaths paths) {
        throw unsupported("getSdmFileSize");
    }

    @Override
    public void maybeCreateSdc(OutputSecureDexMetadataCompanion output) {
        throw unsupported("maybeCreateSdc");
    }
}
