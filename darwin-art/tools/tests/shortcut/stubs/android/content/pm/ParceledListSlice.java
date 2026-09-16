package android.content.pm;

import android.os.Parcel;
import android.os.Parcelable;

/** Test-only stand-in for the framework-owned hidden Parcelable type. */
public final class ParceledListSlice implements Parcelable {
    public static final Parcelable.Creator<ParceledListSlice> CREATOR =
            new Parcelable.Creator<ParceledListSlice>() {
                @Override
                public ParceledListSlice createFromParcel(Parcel source) {
                    return (ParceledListSlice) source.readTypedObject(CREATOR);
                }

                @Override
                public ParceledListSlice[] newArray(int size) {
                    return new ParceledListSlice[size];
                }
            };

    private static final ParceledListSlice EMPTY = new ParceledListSlice();

    private ParceledListSlice() {}

    public static ParceledListSlice emptyList() {
        return EMPTY;
    }

    public boolean isEmpty() {
        return true;
    }
}
