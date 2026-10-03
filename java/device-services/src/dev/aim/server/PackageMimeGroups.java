package dev.aim.server;

import android.os.Parcel;
import java.util.Map;
import java.util.Objects;
import java.util.Set;

/** Original MIME map/set order, including nullable names and type members. */
public final class PackageMimeGroups {
    private PackageMimeGroups() {}
    public static void write(Parcel out, Map<String, Set<String>> groups) {
        Objects.requireNonNull(groups);
        out.writeInt(groups.size());
        for (var group : groups.entrySet()) {
            out.writeString(group.getKey());
            var types = Objects.requireNonNull(group.getValue());
            out.writeInt(types.size());
            for (String type : types) out.writeString(type);
        }
    }
}
