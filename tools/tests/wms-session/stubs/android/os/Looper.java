package android.os;

public final class Looper {
    final HandlerThread owner;
    Looper(HandlerThread value) { owner = value; }
}
