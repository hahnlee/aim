// Compile-only image API; checked by the device-services build node.
package com.android.modules.utils;
public interface TypedXmlPullParser extends org.xmlpull.v1.XmlPullParser {
    float getAttributeFloat(String namespace, String name);
}
