package android.accounts;
public final class Account implements android.os.Parcelable {
    public final String name, type;
    public Account(String name, String type) { this.name = name; this.type = type; }
    public static final android.os.Parcelable.Creator<Account> CREATOR = null;
    public void writeToParcel(android.os.Parcel p, int flags) { throw new AssertionError(); }
    public int describeContents() { return 0; }
}
