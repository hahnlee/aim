package com.android.server.pm;
public class PackageManagerShellCommand extends android.os.ShellCommand {
 private final com.android.server.pm.verify.domain.DomainVerificationShell domains;
 public PackageManagerShellCommand(android.content.pm.IPackageManager p,android.content.Context c,com.android.server.pm.verify.domain.DomainVerificationShell d){domains=d;}
 public void onHelp(){} public int onCommand(String c){Boolean r=domains.runCommand(this,c);return r==null?handleDefaultCommands(c):r?0:1;}
}
