package dev.aim.server;

import android.os.Parcel;
import com.android.server.pm.pkg.SharedLibrary;
import com.android.server.pm.pkg.SharedLibraryWrapper;
import java.util.List;

/** Original owner parcels retain fields hidden by immutable interface getters. */
public final class PackageLibraryFeed {
    public static void write(Parcel out, List<SharedLibrary> libraries) {
        out.writeInt(libraries.size());
        for (var library : libraries) {
            if (!(library instanceof SharedLibraryWrapper wrapper)) throw new IllegalArgumentException("unsupported library owner");
            Parcel parcel = Parcel.obtain();
            try {
                wrapper.getInfo().writeToParcel(parcel, 0);
                out.writeByteArray(parcel.marshall());
            } finally { parcel.recycle(); }
        }
    }
}
