// Compile-only image API; checked by the device-services build node.
package com.android.modules.utils;
public interface TypedXmlPullParser extends org.xmlpull.v1.XmlPullParser {
    boolean getAttributeBoolean(String namespace, String name, boolean defaultValue);
    int getAttributeIntHex(String namespace, String name, int defaultValue);
    long getAttributeLong(String namespace, String name);
    float getAttributeFloat(String namespace, String name);
    float getAttributeFloat(String namespace, String name, float defaultValue);
    long getAttributeLongHex(String namespace, String name, long defaultValue);
}
