package com.android.internal.util;
public @interface DataClass {
    boolean genHiddenConstructor();boolean genAidl();boolean genEqualsHashCode();
    @interface ParcelWith {Class<?> value();}
    @interface Generated {long time();String codegenVersion();String sourceFile();String inputSignatures();@interface Member {}}
}
