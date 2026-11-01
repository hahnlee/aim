"""Extract pinned Java writers; scalar Parcel/Binder substitutes, no Android boot.

Copyright (C) The Android Open Source Project. Apache License 2.0.
Adaptation: only writer methods run; unrelated APIs and native transport are fixtures.
Usage: python3 oracle.py SOURCE_DIRECTORY JDK_HOME OUTPUT_DIRECTORY
"""
import hashlib
from pathlib import Path
import re
import subprocess
import sys

source, jdk, output = map(Path, sys.argv[1:])
output.mkdir(parents=True, exist_ok=True)
pins = {
    'WindowContainerTransaction': 'a95d3bda04237d156e028781d7c7d66d57645a64fd17d1f21bc6de6137f43d3d',
    'Configuration': '75bfabc9cba9b2f18404fddf03dbc0a7375e23594a17deec90a548925fe00401',
    'WindowConfiguration': '10bb467c4fe0180b0cec09c6dcafea53551376def221428f5c1024a12b456334',
    'Parcel': '1bbc6bd44b8fbb178939ad5122725d73486aaa63585c229135535604475fec5a',
}
sources = {}
for name, sha in pins.items():
    data = (source / (name + '.java')).read_bytes()
    assert hashlib.sha256(data).hexdigest() == sha, name
    sources[name] = data.decode()

def method(name, signature, occurrence=0):
    text = sources[name]
    start = [m.start() for m in re.finditer(re.escape(signature), text)][occurrence]
    brace = text.index('{', start)
    depth, end = 1, brace + 1
    while depth:
        depth += (text[end] == '{') - (text[end] == '}')
        end += 1
    return re.sub(r'@(NonNull|Nullable)\s*', '', text[start:end])

writer = lambda n, i=0: method(n, 'public void writeToParcel(', i)
config = writer('Configuration')
ints = ','.join(re.findall(r'dest.writeInt\(([A-Za-z]\w*)\)', config))
# All five original object writers and Parcel's map/value/typed-object framing.
java = '''package android.window;
import java.util.*;
import java.nio.*;
import java.nio.file.*;
public class WindowContainerTransaction implements Parcelable {
 Map<String,Object> mChanges = new LinkedHashMap<>(); List<HierarchyOp> mHierarchyOps = new ArrayList<>();
 Object mErrorCallbackToken, mTaskFragmentOrganizer;
 @WCT@
 static class Change implements Parcelable {
  Configuration mConfiguration = new Configuration();
  boolean mFocusable=true, mHidden, mIgnoreOrientationRequest, mForceTranslucent, mDragResizing, mConfigAtTransitionEnd;
  int mChangeMask, mConfigSetMask=0x20000000, mWindowSetMask=1, mWindowingMode=-1, mActivityWindowingMode=-1;
  Parcelable mPinnedBounds, mBoundsChangeTransaction, mRelativeBounds;
  @CHANGE@
 }
 static class HierarchyOp implements Parcelable {
  int mType=1, mExcludeInsetsTypes; Object mContainer=64, mReparent=64, mInsetsFrameOwner;
  Object[] mContainers; int[] mWindowingModes, mActivityTypes;
  Parcelable mBounds, mInsetsFrameProvider, mLaunchOptions, mAppCompatOptions, mActivityIntent, mTaskFragmentOperation, mKeyguardState, mPendingIntent, mShortcutInfo, mSafeRegionBounds;
  boolean mIncludingParents, mToTop=true, mReparentTopOnly, mAlwaysOnTop, mReparentLeafTaskIfRelaunch, mIsTrimmableFromRecents, mLaunchAdjacentDisabled;
  @HIERARCHY@
 }
 public static void main(String[] args) throws Exception {
  WindowContainerTransaction t = new WindowContainerTransaction();
  ((Map)t.mChanges).put(64, new Change()); t.mHierarchyOps.add(new HierarchyOp());
  Parcel p = new Parcel(); t.writeToParcel(p, 0);
  byte[] data = Arrays.copyOf(p.b.array(), p.b.position());
  Files.write(Path.of(args[0]), data); System.out.println(p.objects);
 }
}
interface Parcelable { void writeToParcel(Parcel p, int flags); }
class Configuration implements Parcelable {
 float fontScale; int @INTS@; boolean userSetLocale;
 Parcelable mLocaleList=(p,f)->p.writeString8(""); WindowConfiguration windowConfiguration=new WindowConfiguration();
 void fixUpLocaleList() {} @CONFIG@
}
class WindowConfiguration implements Parcelable {
 Rect mBounds=new Rect(-80,120,720,1120), mMaxBounds=new Rect(0,0,0,0), mAppBounds;
 int mWindowingMode,mActivityType,mAlwaysOnTop,mRotation=-1,mDisplayRotation=-1; @WINDOW@
}
record Rect(int l,int t,int r,int b) implements Parcelable {
 public void writeToParcel(Parcel p,int f) { p.writeInt(l); p.writeInt(t); p.writeInt(r); p.writeInt(b); }
}
class LazyValue { void writeToParcel(Parcel p) { throw new AssertionError(); } }
class BadParcelableException extends RuntimeException { BadParcelableException(String s) { super(s); } }
class Parcel {
 ByteBuffer b=ByteBuffer.allocate(8192).order(ByteOrder.LITTLE_ENDIAN); List<Integer> objects=new ArrayList<>();
 void writeInt(int v) { b.putInt(v); } void writeFloat(float v) { b.putFloat(v); }
 void writeBoolean(boolean v) { writeInt(v?1:0); } int dataPosition() { return b.position(); }
 void setDataPosition(int p) { b.position(p); }
 void writeString(String s) { writeInt(s==null?-1:s.length()); if(s!=null) { for(char c:s.toCharArray()) b.putChar(c); b.putChar((char)0); while(b.position()%4!=0) b.put((byte)0); } }
 void writeString8(String s) { writeInt(s.length()); b.put(s.getBytes(java.nio.charset.StandardCharsets.UTF_8)); b.put((byte)0); while(b.position()%4!=0)b.put((byte)0); }
 void writeStrongBinder(Object v) { if(v!=null) objects.add(b.position()); writeInt(0x73622a85); writeInt(0); b.putLong(v==null?0:((Number)v).longValue()); b.putLong(0); writeInt(0); }
 void writeStrongInterface(Object v) { writeStrongBinder(v); }
 void writeBinderArray(Object[] v) { if(v!=null)throw new AssertionError(); writeInt(-1); }
 void writeIntArray(int[] v) { if(v!=null)throw new AssertionError(); writeInt(-1); }
 void writeBundle(Parcelable v) { if(v!=null)throw new AssertionError(); writeInt(-1); }
 void writeMap(Map<String,Object> v) { writeMapInternal(v); }
 void writeTypedList(List<WindowContainerTransaction.HierarchyOp> v) { writeTypedList(v,0); }
 int getValueType(Object v) { return v instanceof Parcelable?4:15; }
 boolean isLengthPrefixed(int type) { return type==4; }
 void writeValue(int type,Object v) { if(type==15)writeStrongBinder(v); else writeParcelable((Parcelable)v,0); }
 @PARCEL@
}
'''
replacements = {'WCT': writer('WindowContainerTransaction'), 'CHANGE': writer('WindowContainerTransaction', 1),
                'HIERARCHY': writer('WindowContainerTransaction', 2), 'CONFIG': config,
                'WINDOW': writer('WindowConfiguration'), 'INTS': ints,
                'PARCEL': '\n'.join(method('Parcel', signature) for signature in [
                    'void writeMapInternal(', 'public final void writeValue(@Nullable Object',
                    'public <T extends Parcelable> void writeTypedList(',
                    'public final <T extends Parcelable> void writeTypedObject(',
                    'public final void writeParcelable(', 'public final void writeParcelableCreator('])}
for key, value in replacements.items():
    java = java.replace('@' + key + '@', value)
path = output / 'WindowContainerTransaction.java'
path.write_text(java)
subprocess.run([str(jdk/'bin/javac'), '--release', '17', '-d', str(output), str(path)], check=True)
subprocess.run([str(jdk/'bin/java'), '-cp', str(output), 'android.window.WindowContainerTransaction', str(output/'wct.bin')], check=True)
data = (output/'wct.bin').read_bytes()
(output/'wct.hex').write_text('\n'.join(data[i:i+16].hex() for i in range(0,len(data),16))+'\n')
