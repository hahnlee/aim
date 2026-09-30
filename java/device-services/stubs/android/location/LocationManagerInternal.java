// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.location;

import android.location.util.identity.CallerIdentity;
import android.os.PackageTagsList;

public abstract class LocationManagerInternal {
    public interface ProviderEnabledListener {
        void onProviderEnabledChanged(String provider, int userId, boolean enabled);
    }

    public interface LocationPackageTagsListener {
        void onLocationPackageTagsChanged(int uid, PackageTagsList packageTagsList);
    }

    public LocationManagerInternal() { throw new RuntimeException("stub"); }
    public abstract boolean isProviderEnabledForUser(String provider, int userId);
    public abstract void addProviderEnabledListener(String provider, ProviderEnabledListener listener);
    public abstract void removeProviderEnabledListener(String provider, ProviderEnabledListener listener);
    public abstract boolean isProvider(String provider, CallerIdentity identity);
    public abstract LocationTime getGnssTimeMillis();
    public abstract void setLocationPackageTagsListener(LocationPackageTagsListener listener);
}
