package android.content.pm;
import android.os.*;
import java.util.List;
// Boundary double for the original empty-list parcel, not shipped in the image.
public final class ParceledListSlice<T extends Parcelable> implements Parcelable {
    private final List<T> list;
    public ParceledListSlice(List<T> list) { this.list = list; }
    public void writeToParcel(Parcel out, int flags) {
        if (!list.isEmpty()) throw new AssertionError("unexpected configured provider");
        out.writeInt(list.size());
    }
    public int describeContents() { return 0; }
}
