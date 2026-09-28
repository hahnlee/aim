package dev.aim.probe;

import android.content.res.XmlResourceParser;

import org.xmlpull.v1.XmlPullParserException;

import java.io.IOException;
import java.io.InputStream;
import java.io.Reader;

/** A single, attribute-free linearInterpolator document for the DecorView gate. */
public final class ProbeXmlResourceParser implements XmlResourceParser {
    private int eventType = START_DOCUMENT;

    @Override
    public int next() {
        if (eventType == START_DOCUMENT) {
            eventType = START_TAG;
        } else if (eventType == START_TAG) {
            eventType = END_TAG;
        } else {
            eventType = END_DOCUMENT;
        }
        return eventType;
    }

    @Override
    public int nextToken() {
        return next();
    }

    @Override
    public int nextTag() {
        return next();
    }

    @Override
    public String nextText() {
        if (eventType == START_TAG) {
            eventType = END_TAG;
        }
        return "";
    }

    @Override
    public void require(int type, String namespace, String name)
            throws XmlPullParserException {
        if (eventType != type || (name != null && !name.equals(getName()))) {
            throw new XmlPullParserException("Unexpected probe XML event");
        }
    }

    @Override
    public int getEventType() {
        return eventType;
    }

    @Override
    public String getName() {
        return eventType == START_TAG || eventType == END_TAG
                ? "linearInterpolator"
                : null;
    }

    @Override
    public int getDepth() {
        return eventType == START_TAG || eventType == END_TAG ? 1 : 0;
    }

    @Override
    public String getPositionDescription() {
        return "programmatic linearInterpolator";
    }

    @Override
    public boolean isEmptyElementTag() {
        return true;
    }

    @Override
    public boolean isWhitespace() {
        return false;
    }

    @Override
    public void close() {}

    @Override
    public int getAttributeCount() {
        return 0;
    }

    @Override
    public String getAttributeNamespace(int index) {
        return null;
    }

    @Override
    public String getAttributeName(int index) {
        return null;
    }

    @Override
    public String getAttributePrefix(int index) {
        return null;
    }

    @Override
    public String getAttributeType(int index) {
        return null;
    }

    @Override
    public String getAttributeValue(int index) {
        return null;
    }

    @Override
    public String getAttributeValue(String namespace, String name) {
        return null;
    }

    @Override
    public int getAttributeNameResource(int index) {
        return 0;
    }

    @Override
    public int getAttributeListValue(int index, String[] options, int defaultValue) {
        return defaultValue;
    }

    @Override
    public boolean getAttributeBooleanValue(int index, boolean defaultValue) {
        return defaultValue;
    }

    @Override
    public int getAttributeResourceValue(int index, int defaultValue) {
        return defaultValue;
    }

    @Override
    public int getAttributeIntValue(int index, int defaultValue) {
        return defaultValue;
    }

    @Override
    public int getAttributeUnsignedIntValue(int index, int defaultValue) {
        return defaultValue;
    }

    @Override
    public float getAttributeFloatValue(int index, float defaultValue) {
        return defaultValue;
    }

    @Override
    public int getAttributeListValue(
            String namespace, String attribute, String[] options, int defaultValue) {
        return defaultValue;
    }

    @Override
    public boolean getAttributeBooleanValue(
            String namespace, String attribute, boolean defaultValue) {
        return defaultValue;
    }

    @Override
    public int getAttributeResourceValue(
            String namespace, String attribute, int defaultValue) {
        return defaultValue;
    }

    @Override
    public int getAttributeIntValue(String namespace, String attribute, int defaultValue) {
        return defaultValue;
    }

    @Override
    public int getAttributeUnsignedIntValue(
            String namespace, String attribute, int defaultValue) {
        return defaultValue;
    }

    @Override
    public float getAttributeFloatValue(
            String namespace, String attribute, float defaultValue) {
        return defaultValue;
    }

    @Override
    public String getIdAttribute() {
        return null;
    }

    @Override
    public String getClassAttribute() {
        return null;
    }

    @Override
    public int getIdAttributeResourceValue(int defaultValue) {
        return defaultValue;
    }

    @Override
    public int getStyleAttribute() {
        return 0;
    }

    @Override
    public void defineEntityReplacementText(String entityName, String replacementText) {}

    @Override
    public int getColumnNumber() {
        return -1;
    }

    @Override
    public boolean getFeature(String name) {
        return false;
    }

    @Override
    public String getInputEncoding() {
        return null;
    }

    @Override
    public int getLineNumber() {
        return 1;
    }

    @Override
    public String getNamespace() {
        return null;
    }

    @Override
    public String getNamespace(String prefix) {
        return null;
    }

    @Override
    public int getNamespaceCount(int depth) {
        return 0;
    }

    @Override
    public String getNamespacePrefix(int position) {
        return null;
    }

    @Override
    public String getNamespaceUri(int position) {
        return null;
    }

    @Override
    public String getPrefix() {
        return null;
    }

    @Override
    public Object getProperty(String name) {
        return null;
    }

    @Override
    public String getText() {
        return null;
    }

    @Override
    public char[] getTextCharacters(int[] holderForStartAndLength) {
        holderForStartAndLength[0] = 0;
        holderForStartAndLength[1] = 0;
        return new char[0];
    }

    @Override
    public boolean isAttributeDefault(int index) {
        return false;
    }

    @Override
    public void setFeature(String name, boolean state) throws XmlPullParserException {
        if (state) {
            throw new XmlPullParserException("Unsupported probe XML feature: " + name);
        }
    }

    @Override
    public void setInput(InputStream inputStream, String inputEncoding)
            throws XmlPullParserException {
        throw new XmlPullParserException("Probe parser has a fixed document");
    }

    @Override
    public void setInput(Reader reader) throws XmlPullParserException {
        throw new XmlPullParserException("Probe parser has a fixed document");
    }

    @Override
    public void setProperty(String name, Object value) throws XmlPullParserException {
        throw new XmlPullParserException("Unsupported probe XML property: " + name);
    }
}
