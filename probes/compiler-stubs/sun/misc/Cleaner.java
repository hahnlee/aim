package sun.misc;

import java.lang.ref.PhantomReference;

/** Compile-only Android bootclasspath surface for hidden ART run-tests. */
public class Cleaner extends PhantomReference<Object> {
    private Cleaner(Object referent, Runnable thunk) {
        super(referent, null);
        throw new AssertionError();
    }

    public static Cleaner create(Object referent, Runnable thunk) {
        throw new AssertionError();
    }

    public void clean() { throw new AssertionError(); }
}
