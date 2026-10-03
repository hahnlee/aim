// Compile-only pinned image API; checked by the device-services build node.
package com.android.modules.utils;
public interface TypedXmlSerializer extends org.xmlpull.v1.XmlSerializer {
    org.xmlpull.v1.XmlSerializer attributeLongHex(String namespace, String name, long value);
}
