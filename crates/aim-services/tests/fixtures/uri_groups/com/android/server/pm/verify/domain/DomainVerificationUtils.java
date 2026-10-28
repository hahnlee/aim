package com.android.server.pm.verify.domain;
import android.content.pm.PackageManager.NameNotFoundException;
/** Original validation boundary double, with controlled valid/invalid/error inputs. */
public class DomainVerificationUtils {
    private DomainVerificationUtils() {}
    static boolean isValidDomain(String domain) {
        if (domain.isEmpty()) throw new StringIndexOutOfBoundsException();
        return domain.endsWith(".org");
    }
    static NameNotFoundException throwPackageUnavailable(String name) throws NameNotFoundException {
        throw new NameNotFoundException("Package " + name + " unavailable");
    }
}
