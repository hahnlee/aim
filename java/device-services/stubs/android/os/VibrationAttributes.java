package android.os;
public final class VibrationAttributes implements Parcelable {
    private VibrationAttributes(Parcel parcel) {}
    public static final int FLAG_INVALIDATE_SETTINGS_CACHE = 4;
    public boolean isFlagSet(int flag) { return false; }
    public int getUsage() { return 0; }
    public int getOriginalAudioUsage() { return 0; }
    public int getFlags() { return 0; }
    public void writeToParcel(Parcel p, int flags) {}
    public int describeContents() { return 0; }
    public static final Creator<VibrationAttributes> CREATOR = null;
}
