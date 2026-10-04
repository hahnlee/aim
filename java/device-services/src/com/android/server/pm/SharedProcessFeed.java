package com.android.server.pm;

import android.os.Parcel;
import com.android.server.pm.pkg.SharedUserApi;

/** Capture the original aggregate, including its incremental history. */
public final class SharedProcessFeed {
    public static byte[] capture(SharedUserApi value) {
        if (!((Object)value instanceof SharedUserSetting)) throw new IllegalArgumentException("foreign shared process owner");
        SharedUserSetting owner = (SharedUserSetting)(Object)value;
        Parcel out = Parcel.obtain();
        try {
            out.writeString(value.getName()); out.writeInt(value.getAppId());
            var members = value.getPackageStates(); out.writeInt(members.size());
            for (var member : members) out.writeString(member.getPackageName());
            out.writeInt(owner.processes.size());
            for (var entry : owner.processes.entrySet()) {
                out.writeString(entry.getKey());
                var process = entry.getValue(); out.writeString(process.getName());
                var classes = process.getAppClassNamesByPackage(); out.writeInt(classes.size());
                for (var item : classes.entrySet()) { out.writeString(item.getKey()); out.writeString(item.getValue()); }
                var denied = process.getDeniedPermissions(); out.writeInt(denied.size());
                for (String name : denied) out.writeString(name);
                out.writeInt(process.getGwpAsanMode()); out.writeInt(process.getMemtagMode());
                out.writeInt(process.getNativeHeapZeroInitialized()); out.writeBoolean(process.isUseEmbeddedDex());
            }
            return out.marshall();
        } finally { out.recycle(); }
    }
    private SharedProcessFeed() {}
}
