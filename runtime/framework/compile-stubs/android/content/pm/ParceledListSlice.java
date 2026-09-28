package android.content.pm;

import android.os.Parcel;
import android.os.Parcelable;
import java.util.List;

/** Compile-only Android 16 signature stub; runtime resolution uses framework.jar. */
public class ParceledListSlice<T> implements Parcelable {
    public static final Parcelable.ClassLoaderCreator<ParceledListSlice> CREATOR = null;
    public ParceledListSlice(List<T> list) { throw new RuntimeException("stub"); }
    public static <T> ParceledListSlice<T> emptyList() { throw new RuntimeException("stub"); }
    public List<T> getList() { throw new RuntimeException("stub"); }
    @Override public int describeContents() { throw new RuntimeException("stub"); }
    @Override public void writeToParcel(Parcel dest, int flags) {
        throw new RuntimeException("stub");
    }
}
