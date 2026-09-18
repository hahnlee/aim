package dev.darwinart.runtime.wm;

import android.os.IBinder;
import java.util.Objects;

/**
 * Immutable WMS decision for one desktop-root incarnation.
 *
 * <p>A non-null channel is a grant and carries positive fact/epoch values.
 * A null channel is a revoke; its epoch is retained so a revoke can still
 * order behind an earlier grant even though it has no recipient.</p>
 */
public final class DesktopRootFocusDecision {
    public final long incarnation;
    public final long factSerial;
    public final long sequence;
    public final IBinder originalChannelToken;
    public final long epoch;

    public DesktopRootFocusDecision(long incarnation, long factSerial, long sequence,
            IBinder originalChannelToken, long epoch) {
        if (incarnation == 0L) throw new IllegalArgumentException("incarnation must be nonzero");
        if (sequence <= 0L) throw new IllegalArgumentException("sequence must be positive");
        if (epoch < 0L) throw new IllegalArgumentException("epoch must be nonnegative");
        if (originalChannelToken != null) {
            if (factSerial == 0L) {
                throw new IllegalArgumentException("grant fact serial must be positive");
            }
            if (epoch == 0L) {
                throw new IllegalArgumentException("grant epoch must be positive");
            }
        }
        this.incarnation = incarnation;
        this.factSerial = factSerial;
        this.sequence = sequence;
        this.originalChannelToken = originalChannelToken;
        this.epoch = epoch;
    }

    public long incarnation() { return incarnation; }
    public long factSerial() { return factSerial; }
    public long sequence() { return sequence; }
    public IBinder originalChannelToken() { return originalChannelToken; }
    public long epoch() { return epoch; }

    @Override public boolean equals(Object other) {
        if (this == other) return true;
        if (!(other instanceof DesktopRootFocusDecision)) return false;
        DesktopRootFocusDecision value = (DesktopRootFocusDecision) other;
        return incarnation == value.incarnation
                && factSerial == value.factSerial
                && sequence == value.sequence
                && epoch == value.epoch
                && Objects.equals(originalChannelToken, value.originalChannelToken);
    }

    @Override public int hashCode() {
        return Objects.hash(incarnation, factSerial, sequence, originalChannelToken, epoch);
    }

    @Override public String toString() {
        return "DesktopRootFocusDecision{incarnation=" + incarnation
                + ", factSerial=" + factSerial + ", sequence=" + sequence
                + ", originalChannelToken=" + originalChannelToken + ", epoch=" + epoch + '}';
    }
}
