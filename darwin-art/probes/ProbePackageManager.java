package dev.darwinart.probe;

import android.content.ComponentName;
import android.content.Intent;
import android.content.pm.ActivityInfo;
import android.content.pm.ApplicationInfo;
import android.content.pm.FeatureInfo;
import android.content.pm.PackageManager;
import android.content.pm.PackageInfo;
import android.content.pm.ProviderInfo;
import android.content.pm.ResolveInfo;
import android.content.pm.ServiceInfo;
import android.content.pm.Signature;
import android.content.pm.SigningInfo;
import android.content.res.Resources;
import android.graphics.Color;
import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.Drawable;
import android.os.Bundle;
import android.test.mock.MockPackageManager;
import android.util.TypedValue;
import java.util.HashMap;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import dev.darwinart.runtime.pm.InstalledApplicationInfo;
import static dev.darwinart.runtime.pm.PackageRecords.nativeResolveInstalledPackage;

/** Minimal package policy for framework feature gates before system_server exists. */
public final class ProbePackageManager extends MockPackageManager {
    private static final String GOOGLE_PLAY_SERVICES_PACKAGE = "com.google.android.gms";
    private static final String C2DM_SEND_PERMISSION =
            "com.google.android.c2dm.permission.SEND";
    private static final String IID_TOKEN_REQUEST_ACTION =
            "com.google.iid.TOKEN_REQUEST";
    private String packageName;
    private String activityName;
    private ActivityInfo activityInfo;
    private Resources resources;
    private int versionCode;
    private String versionName;
    private SigningInfo verifiedSigningInfo;
    private boolean signingInfoChecked;
    private Map<ComponentName, Integer> componentEnabledSettings;
    private Map<String, Integer> applicationEnabledSettings;
    private Map<ComponentName, ServiceInfo> serviceInfos;
    private Map<String, Integer> activityThemes;
    private Map<String, String> activityAliases;

    private static String recordValue(String record, String key) {
        if (record == null) return null;
        String prefix = key + "=";
        for (String line : record.split("\\n")) {
            if (line.startsWith(prefix)) return line.substring(prefix.length());
        }
        return null;
    }

    private static String metadataValue(String record, String key) {
        String metadata = recordValue(record, "metadata");
        if (metadata == null) return null;
        String marker = " " + key + "=";
        int start = metadata.indexOf(marker);
        if (start < 0) return null;
        start += marker.length();
        int end = metadata.indexOf(' ', start);
        return end < 0 ? metadata.substring(start) : metadata.substring(start, end);
    }

    private static ApplicationInfo installedApplicationInfo(
            String requestedPackage, String record) {
        ApplicationInfo info = InstalledApplicationInfo.fromRecord(requestedPackage, record);
        if (info == null) return null;
        // The launcher environment describes only the package being run. Do
        // not stamp that label onto records for unrelated installed packages
        // (for example Play Services queried by an SDK).
        String runningPackage = System.getenv("DARWIN_ART_APK_APP_PACKAGE");
        if (runningPackage != null && runningPackage.equals(requestedPackage)) {
            info.nativeLibraryDir = System.getenv("DARWIN_ART_APK_APP_NATIVE_DIR");
            applyApplicationPaths(info);
            applyApplicationLabel(info, null);
            applyApplicationIcon(info);
        }
        return info;
    }

    private static boolean recordDeclaresPermission(String record, String permission) {
        String permissions = metadataValue(record, "permissions");
        if (permissions == null || permission == null) return false;
        for (String declared : permissions.split(",")) {
            if (permission.equals(declared)) return true;
        }
        return false;
    }

    private static ActivityInfo installedActivityInfo(ComponentName component, String record) {
        if (component == null || record == null) return null;
        String requestedName = component.getClassName();
        String resolvedName = requestedName;
        String aliases = metadataValue(record, "activity_aliases");
        if (aliases != null && !"none".equals(aliases)) {
            for (String entry : aliases.split(",")) {
                int separator = entry.indexOf('>');
                if (separator > 0 && requestedName.equals(entry.substring(0, separator))) {
                    resolvedName = entry.substring(separator + 1);
                    break;
                }
            }
        }
        String activities = metadataValue(record, "activities");
        if (activities == null || "none".equals(activities)) return null;
        for (String entry : activities.split(",")) {
            int separator = entry.lastIndexOf('=');
            if (separator <= 0 || !resolvedName.equals(entry.substring(0, separator))) continue;
            ActivityInfo info = new ActivityInfo();
            info.packageName = component.getPackageName();
            info.name = requestedName;
            info.targetActivity = requestedName.equals(resolvedName) ? null : resolvedName;
            info.applicationInfo = installedApplicationInfo(info.packageName, record);
            try {
                info.theme = Long.decode(entry.substring(separator + 1)).intValue();
            } catch (NumberFormatException ignored) {}
            return info;
        }
        return null;
    }

    private static ServiceInfo installedServiceInfo(ComponentName component, String record) {
        if (component == null || record == null) return null;
        String services = metadataValue(record, "services");
        if (services == null || "none".equals(services)) return null;
        for (String entry : services.split(",")) {
            int separator = entry.indexOf('>');
            String name = separator < 0 ? entry : entry.substring(0, separator);
            if (!component.getClassName().equals(name)) continue;
            ServiceInfo info = new ServiceInfo();
            info.packageName = component.getPackageName();
            info.name = name;
            info.processName = separator < 0
                    ? info.packageName : entry.substring(separator + 1);
            info.applicationInfo = installedApplicationInfo(info.packageName, record);
            return info;
        }
        return null;
    }

    public ProbePackageManager() {}

    @Override
    public boolean isPermissionRevokedByPolicy(
            String permission, String packageName) {
        // Runtime permission state is owned by this PackageManager facade;
        // device-policy revocation is false until a DevicePolicyManager
        // explicitly publishes such a restriction.
        return false;
    }

    @Override
    public int checkPermission(String permission, String requestedPackage) {
        String record = nativeResolveInstalledPackage(requestedPackage);
        return recordDeclaresPermission(record, permission)
                ? PackageManager.PERMISSION_GRANTED : PackageManager.PERMISSION_DENIED;
    }

    public boolean shouldShowRequestPermissionRationale(String permission) {
        // No permission request has been denied with "ask again" state in the
        // detached package manager. Android reports false for this initial
        // state; a future permission-controller service can persist denials.
        return false;
    }

    static void applyApplicationPaths(ApplicationInfo info) {
        if (info == null) return;
        String apk = System.getenv("DARWIN_ART_APK_APP_RESOURCE_APK");
        if (apk != null && !apk.isEmpty()) {
            info.sourceDir = apk;
            info.publicSourceDir = apk;
        }
        String splits = System.getenv("DARWIN_ART_APK_APP_SPLIT_SOURCE_DIRS");
        if (splits != null && !splits.isEmpty()) {
            info.splitSourceDirs = splits.split(":");
            info.splitPublicSourceDirs = info.splitSourceDirs.clone();
        }
        String data = System.getenv("DARWIN_ART_APK_APP_DATA_GUEST_DIR");
        if (data != null && !data.isEmpty()) info.dataDir = data;
    }

    /** Applies the selected APK's manifest application label fields. */
    static void applyApplicationLabel(ApplicationInfo info, Resources resources) {
        if (info == null) return;
        String configuredResource = System.getenv("DARWIN_ART_APK_APP_LABEL_RES");
        boolean hasConfiguredResource = false;
        if (configuredResource != null && !configuredResource.isEmpty()) {
            try {
                info.labelRes = Integer.decode(configuredResource).intValue();
                hasConfiguredResource = info.labelRes != 0;
            } catch (NumberFormatException ignored) {
                info.labelRes = 0;
            }
        }
        String configuredLabel = System.getenv("DARWIN_ART_APK_APP_LABEL");
        // ApplicationInfo.loadLabel resolves labelRes through Resources. A
        // resource-backed manifest label must therefore leave
        // nonLocalizedLabel null; otherwise the package fallback would mask
        // the actual localized resource.
        if (hasConfiguredResource) {
            info.nonLocalizedLabel = null;
        } else if (configuredLabel != null && !configuredLabel.isEmpty()) {
            info.nonLocalizedLabel = configuredLabel;
        }
    }

    /** Applies the selected APK's manifest application icon resource ID. */
    static void applyApplicationIcon(ApplicationInfo info) {
        if (info == null) return;
        String runningPackage = System.getenv("DARWIN_ART_APK_APP_PACKAGE");
        if (runningPackage == null || !runningPackage.equals(info.packageName)) return;
        String configured = System.getenv("DARWIN_ART_APK_APP_ICON_RES");
        if (configured == null || configured.isEmpty()) return;
        try {
            info.icon = Integer.decode(configured).intValue();
        } catch (NumberFormatException ignored) {
            info.icon = 0;
        }
    }

    static void applyApplicationMetadata(ApplicationInfo info, Resources resources) {
        String encoded = System.getenv("DARWIN_ART_APK_APP_METADATA");
        if (info == null || encoded == null || encoded.isEmpty()
                || "none".equals(encoded)) return;
        info.metaData = decodeMetadata(encoded, resources);
    }

    private static Bundle decodeMetadata(String encoded, Resources resources) {
        Bundle metadata = new Bundle();
        for (String entry : encoded.split(",")) {
            String[] fields = entry.split(":", -1);
            if (fields.length != 3 || fields[1].length() != 1) continue;
            String name = decodeHexString(fields[0]);
            if (name == null) continue;
            try {
                switch (fields[1].charAt(0)) {
                    case 'r': {
                        int resource = (int) Long.parseLong(fields[2], 16);
                        TypedValue value = new TypedValue();
                        resources.getValue(resource, value, true);
                        if (value.type == TypedValue.TYPE_STRING) {
                            metadata.putString(name, String.valueOf(value.string));
                        } else if (value.type == TypedValue.TYPE_INT_BOOLEAN) {
                            metadata.putBoolean(name, value.data != 0);
                        } else if (value.type >= TypedValue.TYPE_FIRST_INT
                                && value.type <= TypedValue.TYPE_LAST_INT) {
                            metadata.putInt(name, value.data);
                        } else {
                            metadata.putInt(name, resource);
                        }
                        break;
                    }
                    case 'i':
                        metadata.putInt(name, (int) Long.parseLong(fields[2], 16));
                        break;
                    case 'b':
                        metadata.putBoolean(name, "1".equals(fields[2]));
                        break;
                    case 's': {
                        String value = decodeHexString(fields[2]);
                        if (value != null) metadata.putString(name, value);
                        break;
                    }
                    default:
                        break;
                }
            } catch (RuntimeException ignored) {
                // PackageParser skips malformed/unsupported metadata values;
                // preserve the remaining manifest entries.
            }
        }
        return metadata;
    }

    private static String decodeHexString(String encoded) {
        if ((encoded.length() & 1) != 0) return null;
        byte[] bytes = new byte[encoded.length() / 2];
        for (int index = 0; index < bytes.length; index++) {
            int high = Character.digit(encoded.charAt(index * 2), 16);
            int low = Character.digit(encoded.charAt(index * 2 + 1), 16);
            if (high < 0 || low < 0) return null;
            bytes[index] = (byte) ((high << 4) | low);
        }
        try {
            return new String(bytes, "UTF-8");
        } catch (java.io.UnsupportedEncodingException impossible) {
            throw new AssertionError(impossible);
        }
    }

    @Override
    public String[] getPackagesForUid(int uid) {
        // This runtime currently hosts exactly one installed Android package
        // per process, matching PackageManager's caller-identity check used by
        // MediaBrowserService and other Binder services.
        return packageName == null ? null : new String[] {packageName};
    }

    @Override
    public String getNameForUid(int uid) {
        return packageName;
    }

    @Override
    public List<ResolveInfo> queryBroadcastReceivers(Intent intent, int flags) {
        if (intent != null
                && GOOGLE_PLAY_SERVICES_PACKAGE.equals(intent.getPackage())
                && IID_TOKEN_REQUEST_ACTION.equals(intent.getAction())) {
            String record = nativeResolveInstalledPackage(intent.getPackage());
            if (!recordDeclaresPermission(record, C2DM_SEND_PERMISSION)) {
                return Collections.emptyList();
            }
            ResolveInfo resolved = new ResolveInfo();
            resolved.activityInfo = new ActivityInfo();
            resolved.activityInfo.packageName = GOOGLE_PLAY_SERVICES_PACKAGE;
            resolved.activityInfo.name = "com.google.android.gms.gcm.GcmReceiver";
            resolved.activityInfo.permission = C2DM_SEND_PERMISSION;
            resolved.activityInfo.applicationInfo = installedApplicationInfo(
                    GOOGLE_PLAY_SERVICES_PACKAGE, record);
            return Collections.singletonList(resolved);
        }
        return Collections.emptyList();
    }

    @Override
    public List<ResolveInfo> queryBroadcastReceivers(Intent intent,
            PackageManager.ResolveInfoFlags flags) {
        return queryBroadcastReceivers(intent, (int) flags.getValue());
    }

    @Override
    public List<ResolveInfo> queryIntentServices(Intent intent, int flags) {
        ResolveInfo resolved = resolveService(intent, flags);
        return resolved == null
                ? Collections.emptyList() : Collections.singletonList(resolved);
    }

    @Override
    public List<ResolveInfo> queryIntentServices(Intent intent,
            PackageManager.ResolveInfoFlags flags) {
        return Collections.emptyList();
    }

    @Override
    public ResolveInfo resolveService(Intent intent, int flags) {
        if (intent == null || intent.getComponent() == null) return null;
        try {
            ResolveInfo resolved = new ResolveInfo();
            resolved.serviceInfo = getServiceInfo(intent.getComponent(), flags);
            return resolved;
        } catch (PackageManager.NameNotFoundException ignored) {
            return null;
        }
    }

    @Override
    public ResolveInfo resolveService(Intent intent, PackageManager.ResolveInfoFlags flags) {
        return resolveService(intent, (int) flags.getValue());
    }

    @Override
    public ProviderInfo resolveContentProvider(String authority, int flags) {
        return null;
    }

    @Override
    public ProviderInfo getProviderInfo(ComponentName component, int flags)
            throws PackageManager.NameNotFoundException {
        if (component == null || packageName == null
                || !packageName.equals(component.getPackageName())) {
            throw new PackageManager.NameNotFoundException(String.valueOf(component));
        }
        String encoded = System.getenv("DARWIN_ART_APK_APP_PROVIDERS");
        if (encoded == null || encoded.isEmpty() || "none".equals(encoded)) {
            throw new PackageManager.NameNotFoundException(String.valueOf(component));
        }
        for (String item : encoded.split(";")) {
            String[] fields = item.split(">", -1);
            if (fields.length < 4) continue;
            String name = decodeHexString(fields[0]);
            if (!component.getClassName().equals(name)) continue;
            ProviderInfo info = new ProviderInfo();
            info.name = name;
            info.packageName = packageName;
            info.authority = decodeHexString(fields[1]);
            try {
                info.initOrder = (int) Long.parseLong(fields[2], 16);
            } catch (NumberFormatException ignored) {}
            info.applicationInfo = activityInfo == null
                    ? null : new ApplicationInfo(activityInfo.applicationInfo);
            if (!"none".equals(fields[3]) && !fields[3].isEmpty()) {
                info.metaData = decodeMetadata(fields[3], resources);
            }
            return info;
        }
        throw new PackageManager.NameNotFoundException(String.valueOf(component));
    }

    @Override
    public ProviderInfo getProviderInfo(ComponentName component,
            PackageManager.ComponentInfoFlags flags)
            throws PackageManager.NameNotFoundException {
        return getProviderInfo(component, (int) flags.getValue());
    }

    @Override
    public ProviderInfo resolveContentProvider(String authority,
            PackageManager.ComponentInfoFlags flags) {
        return null;
    }

    @Override
    public List<ProviderInfo> queryContentProviders(
            String processName, int uid, int flags) {
        return Collections.emptyList();
    }

    @Override
    public List<ResolveInfo> queryIntentContentProviders(Intent intent, int flags) {
        return Collections.emptyList();
    }

    @Override
    public List<ResolveInfo> queryIntentContentProviders(Intent intent,
            PackageManager.ResolveInfoFlags flags) {
        return Collections.emptyList();
    }

    /** Installs metadata for the unmodified APK selected by the native launcher. */
    public void configure(String packageName, String activityName, String activityNames,
            String activityAliasNames, String serviceNames,
            ActivityInfo source, int targetSdkVersion, Resources resources,
            int versionCode, String versionName) {
        verifiedSigningInfo = null;
        signingInfoChecked = false;
        componentEnabledSettings = new HashMap<>();
        applicationEnabledSettings = new HashMap<>();
        serviceInfos = new HashMap<>();
        activityThemes = new HashMap<>();
        activityAliases = new HashMap<>();
        this.packageName = packageName;
        this.activityName = activityName;
        ActivityInfo configured = source == null ? new ActivityInfo() : source;
        configured.packageName = packageName;
        configured.name = activityName;
        if (System.getenv("DARWIN_ART_APK_APP_PACKAGE") != null) {
            configured.applicationInfo = installedApplicationInfo(packageName,
                    nativeResolveInstalledPackage(packageName));
            if (configured.applicationInfo == null) {
                throw new IllegalStateException("Installed package record missing: " + packageName);
            }
        } else if (configured.applicationInfo == null) {
            configured.applicationInfo = new ApplicationInfo();
        }
        configured.applicationInfo.packageName = packageName;
        configured.applicationInfo.targetSdkVersion = targetSdkVersion;
        configured.applicationInfo.nativeLibraryDir = System.getenv(
                "DARWIN_ART_APK_APP_NATIVE_DIR");
        applyApplicationPaths(configured.applicationInfo);
        applyApplicationLabel(configured.applicationInfo, resources);
        applyApplicationIcon(configured.applicationInfo);
        applyApplicationMetadata(configured.applicationInfo, resources);
        activityInfo = new ActivityInfo(configured);
        if (activityNames != null && !"none".equals(activityNames)) {
            for (String entry : activityNames.split(",")) {
                int separator = entry.lastIndexOf('=');
                if (separator <= 0 || separator == entry.length() - 1) continue;
                String name = entry.substring(0, separator);
                int theme = Long.decode(entry.substring(separator + 1)).intValue();
                activityThemes.put(name, Integer.valueOf(theme));
            }
        }
        if (activityAliasNames != null && !"none".equals(activityAliasNames)) {
            for (String entry : activityAliasNames.split(",")) {
                int separator = entry.indexOf('>');
                if (separator <= 0 || separator == entry.length() - 1) continue;
                activityAliases.put(entry.substring(0, separator),
                        entry.substring(separator + 1));
            }
        }
        if (serviceNames != null && !"none".equals(serviceNames)) {
            for (String serviceSpec : serviceNames.split(",")) {
                int separator = serviceSpec.indexOf('>');
                int flagsSeparator = separator < 0
                        ? -1 : serviceSpec.indexOf('>', separator + 1);
                String serviceName = separator < 0
                        ? serviceSpec : serviceSpec.substring(0, separator);
                String processName = separator < 0
                        ? packageName
                        : flagsSeparator < 0
                                ? serviceSpec.substring(separator + 1)
                                : serviceSpec.substring(separator + 1, flagsSeparator);
                if (serviceName.isEmpty() || processName.isEmpty()) continue;
                ServiceInfo service = new ServiceInfo();
                service.packageName = packageName;
                service.name = serviceName;
                service.processName = processName;
                service.applicationInfo = new ApplicationInfo(activityInfo.applicationInfo);
                if (flagsSeparator >= 0
                        && "1".equals(serviceSpec.substring(flagsSeparator + 1))) {
                    service.flags |= ServiceInfo.FLAG_ISOLATED_PROCESS;
                }
                serviceInfos.put(new ComponentName(packageName, serviceName), service);
            }
        }
        String encodedServiceMetadata = System.getenv(
                "DARWIN_ART_APK_APP_SERVICE_METADATA");
        if (encodedServiceMetadata != null && !encodedServiceMetadata.isEmpty()
                && !"none".equals(encodedServiceMetadata)) {
            for (String entry : encodedServiceMetadata.split(";")) {
                int separator = entry.indexOf('=');
                if (separator <= 0 || separator == entry.length() - 1) continue;
                String serviceName = decodeHexString(entry.substring(0, separator));
                if (serviceName == null) continue;
                ServiceInfo service = serviceInfos.get(
                        new ComponentName(packageName, serviceName));
                if (service != null) {
                    service.metaData = decodeMetadata(
                            entry.substring(separator + 1), resources);
                }
            }
        }
        this.resources = resources;
        this.versionCode = versionCode;
        this.versionName = versionName;
    }

    @Override
    public ServiceInfo getServiceInfo(ComponentName component, int flags)
            throws PackageManager.NameNotFoundException {
        ServiceInfo service = serviceInfos.get(component);
        if (service != null) return new ServiceInfo(service);
        String record = component == null ? null
                : nativeResolveInstalledPackage(component.getPackageName());
        ServiceInfo installed = installedServiceInfo(component, record);
        if (installed != null) return installed;
        throw new PackageManager.NameNotFoundException(String.valueOf(component));
    }

    @Override
    public ServiceInfo getServiceInfo(ComponentName component,
            PackageManager.ComponentInfoFlags flags)
            throws PackageManager.NameNotFoundException {
        return getServiceInfo(component, (int) flags.getValue());
    }

    @Override
    public ActivityInfo getActivityInfo(ComponentName component, int flags)
            throws PackageManager.NameNotFoundException {
        String requestedName = component == null ? null : component.getClassName();
        String targetName = requestedName == null ? null : activityAliases.get(requestedName);
        String resolvedName = targetName == null ? requestedName : targetName;
        if (component != null
                && packageName != null
                && packageName.equals(component.getPackageName())
                && activityThemes.containsKey(resolvedName)) {
            ActivityInfo result = new ActivityInfo(activityInfo);
            result.name = requestedName;
            result.targetActivity = targetName;
            result.theme = activityThemes.get(resolvedName).intValue();
            return result;
        }
        String record = component == null ? null
                : nativeResolveInstalledPackage(component.getPackageName());
        ActivityInfo installed = installedActivityInfo(component, record);
        if (installed != null) return installed;
        throw new PackageManager.NameNotFoundException(String.valueOf(component));
    }

    @Override
    public ActivityInfo getActivityInfo(ComponentName component,
            PackageManager.ComponentInfoFlags flags)
            throws PackageManager.NameNotFoundException {
        return getActivityInfo(component, (int) flags.getValue());
    }

    @Override
    public ActivityInfo getReceiverInfo(ComponentName component, int flags)
            throws PackageManager.NameNotFoundException {
        if (component == null) {
            throw new PackageManager.NameNotFoundException("null");
        }
        String record = nativeResolveInstalledPackage(component.getPackageName());
        String encoded = System.getenv("DARWIN_ART_APK_APP_RECEIVERS");
        if (encoded != null && !encoded.isEmpty() && !"none".equals(encoded)) {
            for (String entry : encoded.split(",")) {
                String[] fields = entry.split(">", -1);
                if (fields.length < 4 || !component.getClassName().equals(fields[0])) continue;
                ActivityInfo info = new ActivityInfo();
                info.packageName = component.getPackageName();
                info.name = fields[0];
                info.processName = fields[1];
                info.enabled = Boolean.parseBoolean(fields[2]);
                info.exported = Boolean.parseBoolean(fields[3]);
                info.applicationInfo = installedApplicationInfo(info.packageName, record);
                return info;
            }
        }
        throw new PackageManager.NameNotFoundException(String.valueOf(component));
    }

    @Override
    public ActivityInfo getReceiverInfo(ComponentName component,
            PackageManager.ComponentInfoFlags flags)
            throws PackageManager.NameNotFoundException {
        return getReceiverInfo(component, (int) flags.getValue());
    }

    @Override
    public ApplicationInfo getApplicationInfo(String requestedPackage, int flags)
            throws PackageManager.NameNotFoundException {
        if (packageName != null && packageName.equals(requestedPackage)) {
            return new ApplicationInfo(activityInfo.applicationInfo);
        }
        ApplicationInfo installed = installedApplicationInfo(
                requestedPackage, nativeResolveInstalledPackage(requestedPackage));
        if (installed != null) return installed;
        throw new PackageManager.NameNotFoundException(requestedPackage);
    }

    @Override
    public ApplicationInfo getApplicationInfo(String requestedPackage,
            PackageManager.ApplicationInfoFlags flags)
            throws PackageManager.NameNotFoundException {
        return getApplicationInfo(requestedPackage, (int) flags.getValue());
    }

    @Override
    public String getInstallerPackageName(String requestedPackage) {
        return null;
    }

    @Override
    public CharSequence getApplicationLabel(ApplicationInfo info) {
        if (info == null || info.packageName == null) {
            throw new IllegalArgumentException("Unknown application");
        }
        if (packageName != null && packageName.equals(info.packageName)) {
            if (info.labelRes != 0 && resources != null) {
                try {
                    return resources.getString(info.labelRes);
                } catch (Resources.NotFoundException ignored) {
                    // Fall through to the literal/package fallback below.
                }
            }
            if (info.nonLocalizedLabel != null) return info.nonLocalizedLabel;
            return packageName;
        }
        String record = nativeResolveInstalledPackage(info.packageName);
        String installed = metadataValue(record, "label");
        if (installed != null) return installed;
        throw new IllegalArgumentException("Unknown application");
    }

    @Override
    public CharSequence getText(String requestedPackage, int resid,
            ApplicationInfo appInfo) {
        if (packageName == null || !packageName.equals(requestedPackage)
                || resources == null || resid == 0) {
            return null;
        }
        try {
            // ActivityInfo/ApplicationInfo.loadLabel routes resource-backed
            // labels through PackageManager.getText(). Keep this scoped to
            // the selected APK; unrelated package resources are unavailable
            // in the detached profile.
            return resources.getText(resid);
        } catch (Resources.NotFoundException ignored) {
            return null;
        }
    }

    private synchronized SigningInfo ownSigningInfo() {
        if (signingInfoChecked) return verifiedSigningInfo;
        if (activityInfo == null || activityInfo.applicationInfo == null) return null;
        signingInfoChecked = true;
        String source = activityInfo.applicationInfo.sourceDir;
        if (source == null) return null;
        try {
            // Use the platform verifier, including APK content digests and
            // signer rotation history. Never synthesize an app certificate.
            Class<?> inputClass = Class.forName("android.content.pm.parsing.result.ParseInput");
            Object input = Class.forName("android.content.pm.parsing.result.ParseTypeImpl")
                    .getMethod("forParsingWithoutPlatformCompat").invoke(null);
            Object result = Class.forName("android.util.apk.ApkSignatureVerifier")
                    .getMethod("verify", inputClass, String.class, int.class)
                    .invoke(null, input, source,
                            activityInfo.applicationInfo.targetSdkVersion >= 30 ? 2 : 1);
            Class<?> resultClass = Class.forName("android.content.pm.parsing.result.ParseResult");
            if ((Boolean) resultClass.getMethod("isError").invoke(result)) {
                Throwable failure = (Throwable) resultClass.getMethod("getException").invoke(result);
                android.util.Log.w("DarwinPackageManager", "APK signature verification failed: "
                        + resultClass.getMethod("getErrorMessage").invoke(result), failure);
                return null;
            }
            Object details = resultClass.getMethod("getResult").invoke(result);
            verifiedSigningInfo = SigningInfo.class.getConstructor(
                    Class.forName("android.content.pm.SigningDetails")).newInstance(details);
            android.util.Log.i("DarwinPackageManager", "APK content and signing identity verified");
        } catch (ReflectiveOperationException | RuntimeException error) {
            android.util.Log.w("DarwinPackageManager", "Cannot verify APK signing identity", error);
        }
        return verifiedSigningInfo;
    }

    @Override
    public PackageInfo getPackageInfo(String requestedPackage, int flags)
            throws PackageManager.NameNotFoundException {
        // Android's framework package is always installed and is addressable
        // through every application PackageManager.  The detached host does
        // not have a registry record for it, so treating it as an ordinary
        // third-party package incorrectly raises NameNotFoundException (VLC's
        // AccessControl and many SDKs query this identity during startup).
        // Keep the identity backed by the actual framework-res artifact when
        // available; do not manufacture a signing certificate.
        if ("android".equals(requestedPackage)) {
            PackageInfo framework = new PackageInfo();
            framework.packageName = "android";
            ApplicationInfo application = new ApplicationInfo();
            application.packageName = "android";
            application.flags = ApplicationInfo.FLAG_SYSTEM;
            String frameworkApk = System.getenv("DARWIN_ART_FRAMEWORK_RES_APK");
            if (frameworkApk != null && !frameworkApk.isEmpty()) {
                application.sourceDir = frameworkApk;
                application.publicSourceDir = frameworkApk;
            }
            framework.applicationInfo = application;
            framework.versionCode = 36;
            framework.setLongVersionCode(36L);
            framework.versionName = "16";
            return framework;
        }
        if (packageName == null || !packageName.equals(requestedPackage)) {
            String record = nativeResolveInstalledPackage(requestedPackage);
            ApplicationInfo installed = installedApplicationInfo(requestedPackage, record);
            if (installed == null) {
                throw new PackageManager.NameNotFoundException(requestedPackage);
            }
            PackageInfo external = new PackageInfo();
            external.packageName = requestedPackage;
            external.applicationInfo = installed;
            String code = metadataValue(record, "version_code");
            if (code != null) {
                try {
                    long parsed = Long.parseLong(code);
                    external.versionCode = (int) parsed;
                    external.setLongVersionCode(parsed);
                } catch (NumberFormatException ignored) {}
            }
            external.versionName = metadataValue(record, "version_name");
            String encodedSignature = metadataValue(record, "signature_base64");
            if (encodedSignature != null) {
                try {
                    Signature signature = new Signature(android.util.Base64.decode(
                            encodedSignature, android.util.Base64.DEFAULT));
                    external.signatures = new Signature[] {signature};
                    external.signingInfo = new SigningInfo(3,
                            Collections.singletonList(signature), null, null);
                } catch (IllegalArgumentException ignored) {
                    // An invalid system registry record must not manufacture a
                    // package signature. Signature checks will reject it.
                }
            }
            return external;
        }
        PackageInfo info = new PackageInfo();
        info.packageName = packageName;
        info.applicationInfo = new ApplicationInfo(activityInfo.applicationInfo);
        info.versionCode = versionCode;
        info.setLongVersionCode(versionCode & 0xffff_ffffL);
        info.versionName = versionName;
        if ((flags & (PackageManager.GET_SIGNATURES
                | PackageManager.GET_SIGNING_CERTIFICATES)) != 0) {
            SigningInfo signing = ownSigningInfo();
            if (signing != null) {
                if ((flags & PackageManager.GET_SIGNING_CERTIFICATES) != 0) {
                    info.signingInfo = new SigningInfo(signing);
                }
                if ((flags & PackageManager.GET_SIGNATURES) != 0) {
                    Signature[] history = signing.getSigningCertificateHistory();
                    info.signatures = history != null && history.length > 0
                            ? new Signature[] {history[0]} : signing.getApkContentsSigners();
                }
            }
        }
        info.activities = new ActivityInfo[activityThemes.size() + activityAliases.size()];
        int index = 0;
        for (Map.Entry<String, Integer> entry : activityThemes.entrySet()) {
            ActivityInfo declared = new ActivityInfo(activityInfo);
            declared.name = entry.getKey();
            declared.theme = entry.getValue().intValue();
            info.activities[index++] = declared;
        }
        for (Map.Entry<String, String> entry : activityAliases.entrySet()) {
            ActivityInfo declared = new ActivityInfo(activityInfo);
            declared.name = entry.getKey();
            declared.targetActivity = entry.getValue();
            Integer targetTheme = activityThemes.get(entry.getValue());
            declared.theme = targetTheme == null ? 0 : targetTheme.intValue();
            info.activities[index++] = declared;
        }
        return info;
    }

    @Override
    public PackageInfo getPackageInfo(String requestedPackage,
            PackageManager.PackageInfoFlags flags)
            throws PackageManager.NameNotFoundException {
        return getPackageInfo(requestedPackage, (int) flags.getValue());
    }

    @Override
    public ResolveInfo resolveActivity(Intent intent, int flags) {
        if (intent == null) return null;
        ComponentName requestedComponent = intent.getComponent();
        if (requestedComponent == null && intent.getPackage() != null) {
            Intent launch = getLaunchIntentForPackage(intent.getPackage());
            requestedComponent = launch == null ? null : launch.getComponent();
        }
        if (requestedComponent == null) return null;
        ComponentName component = requestedComponent;
        String requestedName = component.getClassName();
        String targetName = activityAliases.get(requestedName);
        String resolvedName = targetName == null ? requestedName : targetName;
        if (packageName == null || !packageName.equals(component.getPackageName())) {
            try {
                ResolveInfo installed = new ResolveInfo();
                installed.activityInfo = getActivityInfo(component, flags);
                return installed;
            } catch (PackageManager.NameNotFoundException ignored) {
                return null;
            }
        }
        if (!activityThemes.containsKey(resolvedName)) return null;
        ResolveInfo resolved = new ResolveInfo();
        resolved.activityInfo = new ActivityInfo(activityInfo);
        resolved.activityInfo.name = requestedName;
        resolved.activityInfo.targetActivity = targetName;
        resolved.activityInfo.theme = activityThemes.get(resolvedName).intValue();
        return resolved;
    }

    @Override
    public Intent getLaunchIntentForPackage(String requestedPackage) {
        String record = nativeResolveInstalledPackage(requestedPackage);
        String component = metadataValue(record, "launch_component");
        if (component == null || "none".equals(component)) return null;
        Intent launch = new Intent(Intent.ACTION_MAIN);
        launch.addCategory(Intent.CATEGORY_LAUNCHER);
        launch.setComponent(new ComponentName(requestedPackage, component));
        launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        return launch;
    }

    @Override
    public List<ResolveInfo> queryIntentActivities(Intent intent, int flags) {
        ResolveInfo resolved = resolveActivity(intent, flags);
        return resolved == null
                ? Collections.emptyList() : Collections.singletonList(resolved);
    }

    @Override
    public List<ResolveInfo> queryIntentActivities(Intent intent,
            PackageManager.ResolveInfoFlags flags) {
        return queryIntentActivities(intent, (int) flags.getValue());
    }

    @Override
    public boolean hasSystemFeature(String name) {
        return PackageManager.FEATURE_TOUCHSCREEN.equals(name);
    }

    @Override
    public boolean hasSystemFeature(String name, int version) {
        return hasSystemFeature(name);
    }

    @Override
    public FeatureInfo[] getSystemAvailableFeatures() {
        FeatureInfo touchscreen = new FeatureInfo();
        touchscreen.name = PackageManager.FEATURE_TOUCHSCREEN;
        return new FeatureInfo[] {touchscreen};
    }

    /**
     * Match the framework PackageManager contract for a normally installed
     * APK.  MockPackageManager's default implementation throws "Stub!", but
     * Android libraries (including Play services) use this API as a feature
     * gate during startup.  Profile-installed packages are not instant apps.
     */
    @Override
    public boolean isInstantApp() {
        return false;
    }

    @Override
    public boolean isInstantApp(String requestedPackage) {
        return false;
    }

    @Override
    public int getComponentEnabledSetting(ComponentName component) {
        Integer setting = componentEnabledSettings.get(component);
        return setting == null
                ? PackageManager.COMPONENT_ENABLED_STATE_DEFAULT
                : setting.intValue();
    }

    @Override
    public void setComponentEnabledSetting(ComponentName component, int newState, int flags) {
        if (component == null) {
            throw new IllegalArgumentException("component must not be null");
        }
        componentEnabledSettings.put(component, Integer.valueOf(newState));
    }

    @Override
    public int getApplicationEnabledSetting(String requestedPackage) {
        if (packageName == null || !packageName.equals(requestedPackage)) {
            ApplicationInfo installed = installedApplicationInfo(
                    requestedPackage, nativeResolveInstalledPackage(requestedPackage));
            if (installed == null) {
                throw new IllegalArgumentException("Unknown package: " + requestedPackage);
            }
            return installed.enabled
                    ? PackageManager.COMPONENT_ENABLED_STATE_DEFAULT
                    : PackageManager.COMPONENT_ENABLED_STATE_DISABLED;
        }
        Integer setting = applicationEnabledSettings.get(requestedPackage);
        return setting == null
                ? PackageManager.COMPONENT_ENABLED_STATE_DEFAULT
                : setting.intValue();
    }

    @Override
    public void setApplicationEnabledSetting(String requestedPackage, int newState, int flags) {
        if (packageName == null || !packageName.equals(requestedPackage)) {
            throw new IllegalArgumentException("Unknown package: " + requestedPackage);
        }
        applicationEnabledSettings.put(requestedPackage, Integer.valueOf(newState));
    }

    @Override
    public Drawable getDefaultActivityIcon() {
        if (resources != null) {
            try {
                return resources.getDrawable(android.R.drawable.sym_def_app_icon, null);
            } catch (Resources.NotFoundException ignored) {
                // A transparent framework Drawable still preserves the
                // PhoneWindow icon contract when a reduced resource table has
                // no symbolic default icon.
            }
        }
        return new ColorDrawable(Color.TRANSPARENT);
    }

    @Override
    public Drawable getDrawable(String requestedPackage, int resourceId,
            ApplicationInfo applicationInfo) {
        if (resources == null || resourceId == 0
                || packageName == null || !packageName.equals(requestedPackage)) {
            return null;
        }
        try {
            return resources.getDrawable(resourceId, null);
        } catch (Resources.NotFoundException ignored) {
            return null;
        }
    }
}
