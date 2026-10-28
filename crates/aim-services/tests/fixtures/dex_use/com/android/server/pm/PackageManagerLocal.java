package com.android.server.pm;
public interface PackageManagerLocal {
 default FilteredSnapshot withFilteredSnapshot(int uid,android.os.UserHandle user){throw new AssertionError();}
 interface FilteredSnapshot extends AutoCloseable {
  default com.android.server.pm.pkg.PackageState getPackageState(String name) {return (com.android.server.pm.pkg.PackageState)java.lang.reflect.Proxy.newProxyInstance(getClass().getClassLoader(),new Class<?>[]{com.android.server.pm.pkg.PackageState.class},(p,m,a)->{if(m.getName().equals("getPrimaryCpuAbi"))return null;throw new AssertionError(m);});}
  default void close(){}
 }
}
