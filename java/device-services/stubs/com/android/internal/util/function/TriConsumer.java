// Compile-only pinned image API.
package com.android.internal.util.function;

public interface TriConsumer<A, B, C> {
    void accept(A first, B second, C third);
}
