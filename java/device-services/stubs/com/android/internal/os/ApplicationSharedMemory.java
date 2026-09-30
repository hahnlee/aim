// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.internal.os;

import java.io.FileDescriptor;
import java.io.IOException;

public class ApplicationSharedMemory {
    private ApplicationSharedMemory(FileDescriptor fd, boolean mutable, long ptr) { throw new RuntimeException("stub"); }
    public static ApplicationSharedMemory getInstance() { throw new RuntimeException("stub"); }
    public FileDescriptor getReadOnlyFileDescriptor() throws IOException { throw new RuntimeException("stub"); }
}
