package android.view;

public interface IWindowSession {
    abstract class Stub {
        public static final int TRANSACTION_addToDisplayAsUser = 1;
        public static final int TRANSACTION_relayout = 2;
        public static final int TRANSACTION_remove = 3;
        public static final int TRANSACTION_relayoutAsync = 4;
        public static final int TRANSACTION_setOnBackInvokedCallbackInfo = 5;
    }
}
