// Compile-only pinned image type; never packaged.
package com.android.server.pm.verify.domain;
public class DomainVerificationShell {
    public interface Callback {
        void printState(android.util.IndentingPrintWriter writer, String name, Integer user) throws android.content.pm.PackageManager.NameNotFoundException;
        void printOwnersForPackage(android.util.IndentingPrintWriter writer, String name, Integer user) throws android.content.pm.PackageManager.NameNotFoundException;
        void printOwnersForDomains(android.util.IndentingPrintWriter writer, java.util.List<String> domains, Integer user);
        void setDomainVerificationStatusInternal(String name, int state, android.util.ArraySet<String> domains) throws android.content.pm.PackageManager.NameNotFoundException;
        void setDomainVerificationUserSelectionInternal(int user, String name, boolean enabled, android.util.ArraySet<String> domains) throws android.content.pm.PackageManager.NameNotFoundException;
        void setDomainVerificationLinkHandlingAllowedInternal(String name, boolean allowed, int user) throws android.content.pm.PackageManager.NameNotFoundException;
        void clearDomainVerificationState(java.util.List<String> names);
        void clearUserStates(java.util.List<String> names, int user);
        void verifyPackages(java.util.List<String> names, boolean reVerify);
    }
    public Boolean runCommand(com.android.modules.utils.BasicShellCommandHandler handler, String command) { throw new RuntimeException("stub"); }
    public void printHelp(java.io.PrintWriter writer) { throw new RuntimeException("stub"); }
    public DomainVerificationShell(Callback callback) { throw new RuntimeException("stub"); }
}
