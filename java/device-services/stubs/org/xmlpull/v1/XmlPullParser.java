// Compile-only image API; checked by the device-services build node.
package org.xmlpull.v1;
public interface XmlPullParser {
    int END_DOCUMENT = 1;
    int START_TAG = 2;
    int next() throws java.io.IOException, XmlPullParserException;
    int getDepth();
    String getName();
    String getText();
    String getAttributeValue(String namespace, String name);
}
