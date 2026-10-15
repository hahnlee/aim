package com.android.internal.util;
public class IndentingPrintWriter extends android.util.IndentingPrintWriter {
    public IndentingPrintWriter(java.io.Writer writer, String indent) { super(writer,indent); }
    public IndentingPrintWriter increaseIndent() { throw new RuntimeException("stub"); }
    public IndentingPrintWriter decreaseIndent() { throw new RuntimeException("stub"); }
}
