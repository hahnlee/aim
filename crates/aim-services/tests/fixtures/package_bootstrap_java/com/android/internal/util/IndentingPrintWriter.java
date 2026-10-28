package com.android.internal.util;
public class IndentingPrintWriter extends java.io.PrintWriter {
    public IndentingPrintWriter(java.io.Writer writer, String indent) { super(writer); }
    public IndentingPrintWriter increaseIndent() { return this; }
    public IndentingPrintWriter decreaseIndent() { return this; }
}
