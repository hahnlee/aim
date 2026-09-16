package android.content.pm;

import android.os.Parcel;
import android.os.Parcelable;

public final class ParceledListSlice implements Parcelable {
    public static final Parcelable.Creator<ParceledListSlice> CREATOR =
            new Parcelable.Creator<ParceledListSlice>() {
                public ParceledListSlice createFromParcel(Parcel source) { return null; }
                public ParceledListSlice[] newArray(int size) { return new ParceledListSlice[size]; }
            };
    private static final ParceledListSlice EMPTY = new ParceledListSlice();
    private ParceledListSlice() {}
    public static ParceledListSlice emptyList() { return EMPTY; }
}
