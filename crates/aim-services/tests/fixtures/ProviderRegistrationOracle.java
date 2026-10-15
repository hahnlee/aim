public final class ProviderRegistrationOracle {
    public static void main(String[] args) {
        try {
            var directory=new java.io.File(args[0]);
            var resolver=new com.android.server.pm.resolution.ComponentResolver(null,null);
            var previous=new java.util.HashMap<String,com.android.server.pm.pkg.AndroidPackage>();
            for(int i=0;i<4;i++) {
                byte[] input=java.nio.file.Files.readAllBytes(new java.io.File(directory,"raw-"+i).toPath());
                var pkg=(com.android.internal.pm.parsing.pkg.PackageImpl)com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(input);
                var actual=(com.android.server.pm.pkg.AndroidPackage)(Object)pkg;
                var old=previous.put(actual.getPackageName(),actual);
                if(old!=null) resolver.removeAllComponents(old,false);
                resolver.addAllComponents((com.android.server.pm.pkg.AndroidPackage)(Object)pkg,false,null,null);
                java.nio.file.Files.write(new java.io.File(directory,"original-"+i).toPath(),com.android.server.pm.parsing.PackageCacher.toCacheEntryStatic(pkg));
                var providers=((com.android.server.pm.pkg.AndroidPackage)(Object)pkg).getProviders();
                for(var provider:providers) {
                    var registered=resolver.getProvider(new android.content.ComponentName(actual.getPackageName(),provider.getName()));
                    if(registered==null || !java.util.Objects.equals(registered.getAuthority(),provider.getAuthority()) || registered.isSyncable()!=provider.isSyncable()) throw new AssertionError("original declaration provider differs");
                }
            }
            System.out.println("ORIGINAL_PROVIDER_REGISTRATION 4");System.exit(0);
        } catch(Throwable failure) {failure.printStackTrace(System.out);System.exit(1);}
    }
}
