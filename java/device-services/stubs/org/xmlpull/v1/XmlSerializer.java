// Compile-only pinned image API; checked by the device-services build node.
package org.xmlpull.v1;
public interface XmlSerializer {
    void setOutput(java.io.OutputStream output, String encoding) throws java.io.IOException;
    void startDocument(String encoding, Boolean standalone) throws java.io.IOException;
    XmlSerializer attribute(String namespace, String name, String value) throws java.io.IOException;
    void endDocument() throws java.io.IOException;
    XmlSerializer startTag(String namespace, String name) throws java.io.IOException;
    XmlSerializer endTag(String namespace, String name) throws java.io.IOException;
}
