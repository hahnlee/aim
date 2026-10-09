// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content;

import android.os.IBinder;

public class Intent implements android.os.Parcelable {
    public Intent putStringArrayListExtra(String name, java.util.ArrayList<String> value) { throw new RuntimeException("stub"); }
    public static final int FLAG_INCLUDE_STOPPED_PACKAGES=32;
    public static String ACTION_PACKAGE_REPLACED;
    public static String ACTION_MY_PACKAGE_REPLACED;
    public interface CommandOptionHandler { boolean handleOption(String option, android.os.ShellCommand command); }
    public static Intent parseCommandArgs(android.os.ShellCommand command,CommandOptionHandler handler) throws java.net.URISyntaxException {throw new RuntimeException("stub");}

    public boolean isImplicitImageCaptureIntent() { throw new RuntimeException("stub"); }
    public boolean isWebIntent() { throw new RuntimeException("stub"); }
    public int getFlags() { throw new RuntimeException("stub"); }
    public String getPackage() { throw new RuntimeException("stub"); }
    public ComponentName getComponent() { throw new RuntimeException("stub"); }
    public static final android.os.Parcelable.Creator<Intent> CREATOR = null;
    public void writeToParcel(android.os.Parcel parcel, int flags) { throw new RuntimeException("stub"); }
    public int describeContents() { throw new RuntimeException("stub"); }
    public Intent putExtra(String name, boolean value) { throw new RuntimeException("stub"); }
    public Intent putExtra(String name, android.os.Parcelable value) { throw new RuntimeException("stub"); }
    public static final String ACTION_SCREEN_OFF = "android.intent.action.SCREEN_OFF";
    public static final String ACTION_SCREEN_ON = "android.intent.action.SCREEN_ON";
    public static final String ACTION_PACKAGE_CHANGED = "android.intent.action.PACKAGE_CHANGED";
    public static final String ACTION_PACKAGE_REMOVED = "android.intent.action.PACKAGE_REMOVED";
    public static final String ACTION_PACKAGE_RESTARTED = "android.intent.action.PACKAGE_RESTARTED";
    public static final String ACTION_QUERY_PACKAGE_RESTART = "android.intent.action.QUERY_PACKAGE_RESTART";
    public static final String EXTRA_PACKAGES = "android.intent.extra.PACKAGES";
    public static final String EXTRA_CHANGED_COMPONENT_NAME_LIST = "android.intent.extra.changed_component_name_list";
    public static final String EXTRA_PACKAGE_NAME = "android.intent.extra.PACKAGE_NAME";
    public static final String ACTION_MAIN = "android.intent.action.MAIN";
    public static final String ACTION_DOCK_EVENT = "android.intent.action.DOCK_EVENT";
    public static final String ACTION_BATTERY_CHANGED = "android.intent.action.BATTERY_CHANGED";
    public static final String ACTION_SETTING_RESTORED = "android.os.action.SETTING_RESTORED";
    public static final String ACTION_SHUTDOWN = "android.intent.action.ACTION_SHUTDOWN";
    public static final String ACTION_DREAMING_STARTED = "android.intent.action.DREAMING_STARTED";
    public static final String ACTION_TIME_CHANGED = "android.intent.action.TIME_SET";
    public static final String ACTION_TIMEZONE_CHANGED = "android.intent.action.TIMEZONE_CHANGED";
    public static final String EXTRA_DOCK_STATE = "android.intent.extra.DOCK_STATE";
    public static final int EXTRA_DOCK_STATE_UNDOCKED = 0;
    public static final String EXTRA_SETTING_NAME = "setting_name";
    public static final int FLAG_RECEIVER_FOREGROUND = 0x10000000;
    public static final int FLAG_ACTIVITY_NEW_TASK = 0x10000000;
    public static final int FLAG_ACTIVITY_RESET_TASK_IF_NEEDED = 0x00200000;

    public Intent() { throw new RuntimeException("stub"); }
    public Intent(Intent o) { throw new RuntimeException("stub"); }
    public Intent(String action) { throw new RuntimeException("stub"); }
    public int getIntExtra(String name, int defaultValue) { throw new RuntimeException("stub"); }
    public Intent putExtra(String name, int value) { throw new RuntimeException("stub"); }
    public Intent putExtra(String name, String value) { throw new RuntimeException("stub"); }
    public Intent addFlags(int flags) { throw new RuntimeException("stub"); }
    public Intent setFlags(int flags) { throw new RuntimeException("stub"); }
    public Intent setData(android.net.Uri data) { throw new RuntimeException("stub"); }
    public Intent addCategory(String category) { throw new RuntimeException("stub"); }
    public Intent setClassName(String packageName, String className) { throw new RuntimeException("stub"); }
    public String getAction() { throw new RuntimeException("stub"); }
    public String getStringExtra(String name) { throw new RuntimeException("stub"); }
    public boolean getBooleanExtra(String name, boolean defaultValue) { throw new RuntimeException("stub"); }
    public boolean hasExtra(String name) { throw new RuntimeException("stub"); }
    public <T> T getParcelableExtra(String name, Class<T> type) { throw new RuntimeException("stub"); }
    public <T extends android.os.Parcelable> T getParcelableExtra(String name) { throw new RuntimeException("stub"); }
    public Intent putExtras(android.os.Bundle extras) { throw new RuntimeException("stub"); }
    public android.net.Uri getData() { throw new RuntimeException("stub"); }
    public String[] getStringArrayExtra(String name) { throw new RuntimeException("stub"); }
    public IBinder getIBinderExtra(String name) { throw new RuntimeException("stub"); }
    public Intent setPackage(String packageName) { throw new RuntimeException("stub"); }
    public Intent setComponent(ComponentName component) { throw new RuntimeException("stub"); }
    public Intent putExtra(String name, IBinder value) { throw new RuntimeException("stub"); }
    public Intent putExtra(String name, String[] value) { throw new RuntimeException("stub"); }
    public Intent putExtra(String name, int[] value) { throw new RuntimeException("stub"); }
    public Intent(String action, android.net.Uri uri) { throw new RuntimeException("stub"); }
    public static String EXTRA_UID;
    public static String EXTRA_USER_HANDLE;
    public static String EXTRA_USER;
    public static String EXTRA_VISIBILITY_ALLOW_LIST;
    public static String EXTRA_CHANGED_COMPONENT_NAME;
    public static String EXTRA_DONT_KILL_APP;
    public static String EXTRA_REASON;
    public static String EXTRA_TIME;
    public static String EXTRA_DATA_REMOVED;
    public static String EXTRA_SYSTEM_UPDATE_UNINSTALL;
    public static String EXTRA_USER_INITIATED;
    public static String EXTRA_REMOVED_FOR_ALL_USERS;
    public static String EXTRA_ARCHIVAL;
    public static String EXTRA_CHANGED_PACKAGE_LIST;
    public static String EXTRA_CHANGED_UID_LIST;
    public static String EXTRA_QUARANTINED;
    public static String EXTRA_SUSPENDED_PACKAGE_EXTRAS;
    public static String EXTRA_DISTRACTION_RESTRICTIONS;
    public static String ACTION_PACKAGE_FIRST_LAUNCH;
    public static String ACTION_PACKAGE_UNSTOPPED;
    public static String ACTION_PACKAGE_REMOVED_INTERNAL;
    public static String ACTION_PACKAGE_ADDED;
    public static String ACTION_LOCKED_BOOT_COMPLETED;
    public static String ACTION_BOOT_COMPLETED;
    public static String ACTION_PACKAGES_SUSPENSION_CHANGED;
    public static String ACTION_PACKAGES_SUSPENDED;
    public static String ACTION_PACKAGES_UNSUSPENDED;
    public static String ACTION_MY_PACKAGE_SUSPENDED;
    public static String ACTION_MY_PACKAGE_UNSUSPENDED;
    public static String ACTION_DISTRACTING_PACKAGES_CHANGED;
    public static int FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT;
    public static int FLAG_RECEIVER_REGISTERED_ONLY;
    public static int FLAG_RECEIVER_INCLUDE_BACKGROUND;
    public Intent putExtra(String name,long value){throw new RuntimeException("stub");}
 public String resolveTypeIfNeeded(ContentResolver resolver){throw new RuntimeException("stub");}
 public static final String ACTION_VIEW="android.intent.action.VIEW",ACTION_EDIT="android.intent.action.EDIT",ACTION_SEND="android.intent.action.SEND",ACTION_SENDTO="android.intent.action.SENDTO",ACTION_SEND_MULTIPLE="android.intent.action.SEND_MULTIPLE",ACTION_OVERLAY_CHANGED="android.intent.action.OVERLAY_CHANGED",ACTION_UNINSTALL_PACKAGE="android.intent.action.UNINSTALL_PACKAGE",ACTION_UNARCHIVE_PACKAGE="android.intent.action.UNARCHIVE_PACKAGE",ACTION_PACKAGE_FULLY_REMOVED="android.intent.action.PACKAGE_FULLY_REMOVED",ACTION_UID_REMOVED="android.intent.action.UID_REMOVED",EXTRA_INTENT="android.intent.extra.INTENT";
 public static final String EXTRA_REPLACING="android.intent.extra.REPLACING";
}
