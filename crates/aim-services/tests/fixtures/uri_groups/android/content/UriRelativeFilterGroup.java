package android.content;
import java.util.ArrayList;
import java.util.List;
/** Conversion boundary double: records original-class dispatch and injects failure. */
public class UriRelativeFilterGroup {
    public static int conversions;
    private final UriRelativeFilterGroupParcel parcel;
    private UriRelativeFilterGroup(UriRelativeFilterGroupParcel parcel) {
        if (parcel == null) throw new NullPointerException("original conversion failure");
        this.parcel = parcel;
    }
    public static List<UriRelativeFilterGroup> parcelsToGroups(List<UriRelativeFilterGroupParcel> parcels) {
        conversions++;
        var groups = new ArrayList<UriRelativeFilterGroup>();
        if (parcels != null) for (var parcel : parcels) groups.add(new UriRelativeFilterGroup(parcel));
        return groups;
    }
    public static List<UriRelativeFilterGroupParcel> groupsToParcels(List<UriRelativeFilterGroup> groups) {
        var parcels = new ArrayList<UriRelativeFilterGroupParcel>();
        for (var group : groups) parcels.add(group.parcel);
        return parcels;
    }
}
