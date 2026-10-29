package com.android.server.pm.parsing;
public class PackageCacher {public static Object fromCacheEntryStatic(byte[] b){
 return java.lang.reflect.Proxy.newProxyInstance(PackageCacher.class.getClassLoader(),new Class[]{com.android.server.pm.pkg.AndroidPackage.class},(p,m,a)->{
 if(m.getName().equals("getSigningDetails"))return new android.content.pm.SigningDetails();throw new AssertionError(m.getName());});
}}
