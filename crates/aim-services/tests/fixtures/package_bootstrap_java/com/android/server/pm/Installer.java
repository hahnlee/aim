// Copyright (C) The Android Open Source Project, Apache License 2.0.
package com.android.server.pm;
import android.os.CreateAppDataArgs;
import android.os.CreateAppDataResult;
import java.util.concurrent.CompletableFuture;
public class Installer {
    public Installer(android.content.Context context) {}
    public CreateAppDataResult createAppData(CreateAppDataArgs args) throws InstallerException { throw new AssertionError(); }
    public CreateAppDataResult[] createAppDataBatched(CreateAppDataArgs[] args) throws InstallerException { throw new AssertionError(); }
    public void destroyAppData(String uuid, String name, int user, int flags, long inode) throws InstallerException { throw new AssertionError(); }
    public void linkNativeLibraryDirectory(String uuid, String name, String path, int user) throws InstallerException { throw new AssertionError(); }
    public static class InstallerException extends Exception {
        public InstallerException(String message) { super(message); }
    }
    // Original Installer.Batch, restricted to the one-package fixture.
    public static class Batch {
        private CreateAppDataArgs args;
        private final CompletableFuture<CreateAppDataResult> future = new CompletableFuture<>();
        public CompletableFuture<CreateAppDataResult> createAppData(CreateAppDataArgs args) {
            this.args = args; return future;
        }
        public void execute(Installer installer) throws InstallerException {
            var result = installer.createAppDataBatched(new CreateAppDataArgs[] {args})[0];
            if (result.exceptionCode == 0) future.complete(result);
            else future.completeExceptionally(new InstallerException(result.exceptionMessage));
        }
    }
}
