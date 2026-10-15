// Compile-only pinned original API; never included in guest output.
package android.content.pm;
public final class ModuleInfo implements android.os.Parcelable {
    public ModuleInfo() {}
    public ModuleInfo(ModuleInfo other) {}
    public static final android.os.Parcelable.Creator<ModuleInfo> CREATOR = null;
    public CharSequence getName() {throw new RuntimeException("stub");}
    public String getPackageName() {throw new RuntimeException("stub");}
    public boolean isHidden() {throw new RuntimeException("stub");}
    public String getApexModuleName() {throw new RuntimeException("stub");}
    public java.util.Collection<String> getApkInApexPackageNames() {throw new RuntimeException("stub");}
    public ModuleInfo setName(CharSequence name) {throw new RuntimeException("stub");}
    public ModuleInfo setPackageName(String name) {throw new RuntimeException("stub");}
    public ModuleInfo setHidden(boolean hidden) {throw new RuntimeException("stub");}
    public ModuleInfo setApexModuleName(String name) {throw new RuntimeException("stub");}
    public ModuleInfo setApkInApexPackageNames(java.util.Collection<String> names) {throw new RuntimeException("stub");}
    public int describeContents() {return 0;}
    public void writeToParcel(android.os.Parcel parcel,int flags) {throw new RuntimeException("stub");}
}
