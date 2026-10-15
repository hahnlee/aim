package com.android.server.pm;

import android.os.Parcel;
import com.android.server.pm.pkg.SharedUserApi;

/** Explicit diagnostic options from the original typed object; no reflective fields. */
public final class NativeDumpStateRecord {
    private NativeDumpStateRecord() {}
    public static byte[] capture(DumpState state) {
        Parcel out = Parcel.obtain();
        try {
            out.writeInt(1);
            int types = 0, options = 0;
            for (int bit = 0; bit < 31; bit++) if (state.isDumping(1 << bit)) types |= 1 << bit;
            for (int bit = 0; bit < 4; bit++) if (state.isOptionEnabled(1 << bit)) options |= 1 << bit;
            out.writeInt(types); out.writeInt(options); out.writeBoolean(state.getTitlePrinted());
            out.writeString(state.getTargetPackageName()); out.writeBoolean(state.isFullPreferred());
            out.writeBoolean(state.isCheckIn()); out.writeBoolean(state.isBrief());
            var shared = state.getSharedUser(); out.writeBoolean(shared != null);
            if (shared != null) {
                var value = (SharedUserApi) (Object) shared;
                out.writeInt(value.getAppId()); out.writeString(value.getName());
            }
            return out.marshall();
        } finally { out.recycle(); }
    }
}
