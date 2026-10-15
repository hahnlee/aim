// Compile-only pinned image API; no runtime implementation.
package android.content.pm;
public class ResolveInfo implements android.os.Parcelable {
    public void dump(android.util.Printer printer,String prefix){throw new RuntimeException("stub");}

    public static final android.os.Parcelable.Creator<ResolveInfo> CREATOR = null;
    public boolean isInstantAppAvailable;
    public AuxiliaryResolveInfo auxiliaryInfo;
    public android.content.IntentFilter filter;
    public String resolvePackageName;
    public int labelRes, icon;
    public ComponentInfo getComponentInfo() { throw new RuntimeException("stub"); }
    public int resolveLabelResId() { throw new RuntimeException("stub"); }
    public int resolveIconResId() { throw new RuntimeException("stub"); }
    public ActivityInfo activityInfo;
    public int preferredOrder, match, targetUserId;
    public boolean isDefault, noResourceId;
    public android.os.UserHandle userHandle;
    public int priority;
    public ServiceInfo serviceInfo;
    public ProviderInfo providerInfo;
    public ResolveInfo() { throw new RuntimeException("stub"); }
    public ResolveInfo(ResolveInfo other) { throw new RuntimeException("stub"); }
    public void writeToParcel(android.os.Parcel out, int flags) { throw new RuntimeException("stub"); }
    public int describeContents() { throw new RuntimeException("stub"); }
}
