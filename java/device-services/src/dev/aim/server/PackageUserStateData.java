package dev.aim.server;

import android.os.Parcel;
import android.os.PersistableBundle;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;

/** Immutable captured user inputs, including native runtime owners. */
public final class PackageUserStateData {
    private final long version;
    private final String name;
    private final int appId, userId;
    private final boolean factory;
    public final long ceDataInode, deDataInode, firstInstallTime;
    public final boolean installed, stopped, notLaunched, hidden, instantApp, virtualPreload;
    public final int distractionFlags, enabled, installReason, uninstallReason, minAspectRatio, legacyDomainStatus;
    public final String lastDisableCaller, harmfulWarning, splashTheme;
    private final String[] enabledComponents, disabledComponents;
    private final List<Suspension> suspensions;
    public final Archive archive;
    public final Paths overlayPaths;
    private final List<LibraryOverlay> libraryOverlays;
    private final List<LabelIcon> labelIcons;

    static PackageUserStateData read(Parcel in) {
        return new PackageUserStateData(in);
    }
    private PackageUserStateData(Parcel in) {
        version = in.readLong(); name = Objects.requireNonNull(in.readString());
        appId = in.readInt(); factory = in.readBoolean(); userId = in.readInt();
        if (userId < 0) throw new IllegalArgumentException("invalid user id");
        ceDataInode = in.readLong(); deDataInode = in.readLong();
        installed = in.readBoolean(); stopped = in.readBoolean();
        notLaunched = in.readBoolean(); hidden = in.readBoolean();
        distractionFlags = in.readInt(); instantApp = in.readBoolean(); virtualPreload = in.readBoolean();
        enabled = in.readInt(); lastDisableCaller = in.readString();
        enabledComponents = strings(in); disabledComponents = strings(in);
        installReason = in.readInt(); uninstallReason = in.readInt();
        harmfulWarning = in.readString(); splashTheme = in.readString();
        firstInstallTime = in.readLong(); minAspectRatio = in.readInt(); legacyDomainStatus = in.readInt();
        int count = count(in);
        var records = new ArrayList<Suspension>(count);
        for (int i = 0; i < count; i++) records.add(new Suspension(in));
        suspensions = List.copyOf(records);
        archive = in.readBoolean() ? new Archive(in) : null;
        overlayPaths = in.readBoolean() ? new Paths(in) : null;
        if (in.readBoolean()) {
            int size = count(in);
            var values = new ArrayList<LibraryOverlay>(size);
            for (int i = 0; i < size; i++) values.add(new LibraryOverlay(in));
            libraryOverlays = List.copyOf(values);
        } else libraryOverlays = null;
        if (in.readBoolean()) {
            int size = count(in);
            var values = new ArrayList<LabelIcon>(size);
            for (int i = 0; i < size; i++) values.add(new LabelIcon(in));
            labelIcons = List.copyOf(values);
        } else labelIcons = null;
    }
    private static int count(Parcel in) {
        int count = in.readInt();
        if (count < 0 || count > in.dataAvail() / 4) throw new IllegalArgumentException("invalid user state count");
        return count;
    }
    private static String[] strings(Parcel in) {
        int count = in.readInt();
        if (count == -1) return null;
        if (count < 0 || count > in.dataAvail() / 4) throw new IllegalArgumentException("invalid component count");
        String[] values = new String[count];
        for (int i = 0; i < values.length; i++) values[i] = Objects.requireNonNull(in.readString());
        return values;
    }
    public long getVersion() { return version; }
    public String getPackageName() { return name; }
    public int getAppId() { return appId; }
    public int getUserId() { return userId; }
    public boolean isFactory() { return factory; }
    public String[] getEnabledComponents() { return enabledComponents == null ? null : enabledComponents.clone(); }
    public String[] getDisabledComponents() { return disabledComponents == null ? null : disabledComponents.clone(); }
    public List<Suspension> getSuspensions() { return suspensions; }

    public List<LibraryOverlay> getLibraryOverlays() { return libraryOverlays; }
    public List<LabelIcon> getLabelIcons() { return labelIcons; }
    public static final class Paths {
        public final List<String> resourceDirs, overlayPaths;
        private Paths(Parcel in) {
            resourceDirs = List.copyOf(java.util.Arrays.asList(Objects.requireNonNull(strings(in))));
            overlayPaths = List.copyOf(java.util.Arrays.asList(Objects.requireNonNull(strings(in))));
        }
    }
    public static final class LibraryOverlay {
        public final String library;
        public final Paths paths;
        private LibraryOverlay(Parcel in) {
            library = Objects.requireNonNull(in.readString());
            if (!in.readBoolean()) throw new IllegalArgumentException("null library overlay");
            paths = new Paths(in);
        }
    }
    public static final class LabelIcon {
        public final String packageName, className, label;
        public final Integer icon;
        private LabelIcon(Parcel in) {
            packageName = Objects.requireNonNull(in.readString());
            className = Objects.requireNonNull(in.readString());
            label = in.readString(); icon = in.readBoolean() ? in.readInt() : null;
            if (label == null && icon == null) throw new IllegalArgumentException("empty label/icon override");
        }
    }

    public static final class Suspension {
        public final String packageName;
        public final boolean currentUser, quarantined;
        public final Integer storedUser;
        public final Dialog dialog;
        private final byte[] appExtras, launcherExtras;
        private Suspension(Parcel in) {
            packageName = Objects.requireNonNull(in.readString());
            currentUser = in.readBoolean(); storedUser = in.readBoolean() ? in.readInt() : null;
            if (currentUser && storedUser != null) throw new IllegalArgumentException("legacy suspension has stored user");
            quarantined = in.readBoolean(); dialog = in.readBoolean() ? new Dialog(in) : null;
            appExtras = in.createByteArray(); launcherExtras = in.createByteArray();
            decode(appExtras); decode(launcherExtras);
        }
        // Each getter rebuilds the original mutable object from owned bytes;
        // changing its arrays/nested bundles cannot alter this capture.
        public PersistableBundle getAppExtras() { return decode(appExtras); }
        public PersistableBundle getLauncherExtras() { return decode(launcherExtras); }
        private static PersistableBundle decode(byte[] bytes) {
            if (bytes == null) return null;
            Parcel in = Parcel.obtain();
            try {
                in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
                PersistableBundle value = Objects.requireNonNull(in.readPersistableBundle());
                value.keySet();
                if (in.dataAvail() != 0) throw new IllegalArgumentException("trailing suspension extras");
                return value;
            } finally { in.recycle(); }
        }
    }
    public static final class Dialog {
        public final int icon, titleResource, messageResource, buttonResource, buttonAction;
        public final String title, message, button;
        private Dialog(Parcel in) {
            icon = in.readInt(); titleResource = in.readInt(); title = in.readString();
            messageResource = in.readInt(); message = in.readString();
            buttonResource = in.readInt(); button = in.readString(); buttonAction = in.readInt();
        }
    }
    public static final class Archive {
        public final String installerTitle;
        public final long time;
        public final List<ArchiveActivity> activities;
        private Archive(Parcel in) {
            installerTitle = Objects.requireNonNull(in.readString()); time = in.readLong();
            int count = count(in);
            var values = new ArrayList<ArchiveActivity>(count);
            for (int i = 0; i < count; i++) values.add(new ArchiveActivity(in));
            activities = List.copyOf(values);
        }
    }
    public static final class ArchiveActivity {
        public final String title, component, icon, monochromeIcon;
        private ArchiveActivity(Parcel in) {
            title = Objects.requireNonNull(in.readString()); component = Objects.requireNonNull(in.readString());
            icon = Objects.requireNonNull(in.readString()); monochromeIcon = in.readString();
        }
    }
}
