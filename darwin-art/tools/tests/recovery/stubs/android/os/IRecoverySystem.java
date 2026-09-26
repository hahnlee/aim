package android.os;
/** Test stub: the Android 16 transaction order is irrelevant here, only uniqueness. */
public interface IRecoverySystem {
    abstract class Stub {
        static final int TRANSACTION_uncrypt = 1;
        static final int TRANSACTION_setupBcb = 2;
        static final int TRANSACTION_clearBcb = 3;
        static final int TRANSACTION_rebootRecoveryWithCommand = 4;
        static final int TRANSACTION_requestLskf = 5;
        static final int TRANSACTION_clearLskf = 6;
        static final int TRANSACTION_isLskfCaptured = 7;
        static final int TRANSACTION_rebootWithLskfAssumeSlotSwitch = 8;
        static final int TRANSACTION_rebootWithLskf = 9;
        static final int TRANSACTION_allocateSpaceForUpdate = 10;
    }
}
