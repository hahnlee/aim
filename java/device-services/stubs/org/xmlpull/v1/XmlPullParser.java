// Compile-only image API; checked by the device-services build node.
package org.xmlpull.v1;
public interface XmlPullParser {
    int next() throws java.io.IOException;
    int getDepth();
    String getName();
    String getText();
    String getAttributeValue(String namespace, String name);
}
