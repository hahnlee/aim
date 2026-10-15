package dev.aim.server;

import com.android.server.pm.verify.domain.DomainVerificationManagerInternal;
import java.nio.ByteBuffer;
import java.util.Objects;

/** Domain identity remains owned by the original domain verification service. */
public final class PackageDomainIds {
    public static byte[] generate(DomainVerificationManagerInternal owner) {
        if (owner == null) throw new IllegalStateException("domain owner is unavailable");
        var id = Objects.requireNonNull(owner.generateNewId(), "missing domain ID");
        return ByteBuffer.allocate(16).putLong(id.getMostSignificantBits())
            .putLong(id.getLeastSignificantBits()).array();
    }
    private PackageDomainIds() {}
}
