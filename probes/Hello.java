package dev.aim.probe;

public final class Hello extends JitConstructorParent {
    public static int jitInitializationCount;
    public static int jitFailedStaticReadCount, jitFailedStaticWriteCount;
    public static long jitFailedStaticRead() { return JitFailedStaticRead.value; }
    public static void jitFailedStaticWrite(long v) { JitFailedStaticWrite.value = v; }
    public static int jitColdMovingCount;
    public static void jitColdMovingSet(Object v) { JitColdMoving.value = v; }
    public static Object jitColdMovingGet() { return JitColdMoving.value; }
    public static int jitColdLongCount, jitColdDoubleCount, jitColdReferenceCount;
    public static void jitColdLongSet(long v) { JitColdLong.value = v; }
    public static long jitColdLongGet() { return JitColdLong.value; }
    public static void jitColdDoubleSet(double v) { JitColdDouble.value = v; }
    public static double jitColdDoubleGet() { return JitColdDouble.value; }
    public static void jitColdReferenceSet(Object v) { JitColdReference.value = v; }
    public static Object jitColdReferenceGet() { return JitColdReference.value; }
    public static int jitStaticInitializationCount;
    public static int jitColdStaticRead() { return JitColdStatic.value; }
    public static int jitRecursiveInitializationCount;
    public static int jitRecursiveInitializationObserved;
    public static int jitRecursiveInitialization() { return JitRecursiveInitialization.read() + 1; }
    public static Object jitConcurrentInitializationLock;
    public static int jitConcurrentInitializationCount;
    public static int jitConcurrentFailedInitializationCount;
    public static int jitConcurrentFailedInitialization() { return JitConcurrentFailedInitialization.read() + 1; }
    public static int jitConcurrentInitialization() { return JitConcurrentInitialization.read() + 1; }
    public static int jitFailedInitializationCount;
    public static int jitInitializationZero;
    public static int jitFailedInitialization() { return JitFailedInitialization.read() + 1; }
    public static int jitFirstInitialization() { return JitColdInitialization.read() + 1; }
    public static boolean jitIsChecksum(Object v) { return v instanceof java.util.zip.Checksum; }
    public static Object jitCastChecksum(Object v) { return (java.util.zip.Checksum)v; }
    public static boolean jitIsObjects(Object v) { return v instanceof Object[]; }
    public static Object jitCastObjects(Object v) { return (Object[])v; }
    public static boolean jitIsStrings(Object v) { return v instanceof String[]; }
    public static Object jitCastStrings(Object v) { return (String[])v; }
    public static boolean jitIsInts(Object v) { return v instanceof int[]; }
    public static Object jitCastInts(Object v) { return (int[])v; }
    public static boolean jitIsSequences(Object v) { return v instanceof CharSequence[]; }
    public static Object jitCastSequences(Object v) { return (CharSequence[])v; }
    public static boolean jitIsSequence(Object v) { return v instanceof CharSequence; }
    public static Object jitCastSequence(Object v) { return (CharSequence)v; }
    public static boolean jitIsSerializable(Object v) { return v instanceof java.io.Serializable; }
    public static Object jitCastSerializable(Object v) { return (java.io.Serializable)v; }
    public static boolean jitIsBase(Object v) { return v instanceof JitVirtualBase; }
    public static Object jitCastBase(Object v) { return (JitVirtualBase)v; }
    public static boolean jitIsNumber(Object v) { return v instanceof Number; }
    public static Object jitCastNumber(Object v) { return (Number)v; }
    public static boolean jitIsString(Object v) { return v instanceof String; }
    public static Object jitCastString(Object v) { return (String)v; }
    public static int jitPackedSwitch(int v) {
        switch(v) {
            case -4:return 17; case -3:return 31; case -2:return -91; case -1:return 5;
            case 0:return 23; case 1:return -47; case 2:return 101; case 3:return 7;
            case 4:return 89; case 5:return -11; case 6:return 73; case 7:return 41;
            default:return -999;
        }
    }
    public static int jitSparseSwitch(int v) {
        switch(v) {
            case Integer.MIN_VALUE:return 17; case -1000000:return 31; case -7:return -91;
            case 0:return 5; case 13:return 23; case 65535:return -47;
            case Integer.MAX_VALUE:return 101; default:return -999;
        }
    }
    public static Object jitOsrReferences(Object a,Object b,int n) {
        for(int i=0;i<n;i++) { Object tmp=a; a=b; b=tmp; } return a;
    }
    public static int jitOsrDivide(int v,int n) { for(int i=0;i<n;i++) v+=12345/(1000-i); return v; }
    public static int jitAutoLoop(int v,int n) { for(int i=0;i<n;i++) v=(v*31+i)^(v>>>3); return v; }
    public static int jitLoopI(int v,int n) { for(int i=0;i<n;i++) v=(v*31+i)^(v>>>3); return v; }
    public static long jitLoopJ(long v,int n) { for(int i=0;i<n;i++) v=(v*31+i)^(v>>>3); return v; }
    public static float jitLoopF(float v,int n) { for(int i=0;i<n;i++) v=v*0.75f+i; return v; }
    public static double jitLoopD(double v,int n) { for(int i=0;i<n;i++) v=v*0.75+i; return v; }
    public static int jitIntDivide(int a,int b) { return a/b; }
    public static int jitIntRemainder(int a,int b) { return a%b; }
    public static int jitIntDivideSeven(int a,int b) { return a/7; }
    public static int jitIntRemainderSeven(int a,int b) { return a%7; }
    public static int jitIntDivideMinusOne(int a,int b) { return a/-1; }
    public static int jitIntRemainderMinusOne(int a,int b) { return a%-1; }
    public static int jitCompareI(int a,int b) {
        int r=0; if(a==b)r|=1; if(a!=b)r|=2; if(a<b)r|=4;
        if(a<=b)r|=8; if(a>b)r|=16; if(a>=b)r|=32; return r;
    }
    public static int jitCompareJ(long a,long b) {
        int r=0; if(a==b)r|=1; if(a!=b)r|=2; if(a<b)r|=4;
        if(a<=b)r|=8; if(a>b)r|=16; if(a>=b)r|=32; return r;
    }
    public static int jitCompareF(float a,float b) {
        int r=0; if(a==b)r|=1; if(a!=b)r|=2; if(a<b)r|=4;
        if(a<=b)r|=8; if(a>b)r|=16; if(a>=b)r|=32; return r;
    }
    public static int jitCompareD(double a,double b) {
        int r=0; if(a==b)r|=1; if(a!=b)r|=2; if(a<b)r|=4;
        if(a<=b)r|=8; if(a>b)r|=16; if(a>=b)r|=32; return r;
    }
    public static long jitAndLong(long a,long b) { return a&b; }
    public static long jitOrLong(long a,long b) { return a|b; }
    public static long jitXorLong(long a,long b) { return a^b; }
    public static long jitShlLong(long a,int b) { return a<<b; }
    public static long jitShrLong(long a,int b) { return a>>b; }
    public static long jitUshrLong(long a,int b) { return a>>>b; }
    public static byte jitNarrowB(int value) { return (byte)value; }
    public static char jitNarrowC(int value) { return (char)value; }
    public static short jitNarrowS(int value) { return (short)value; }
    public static long jitConvertIJ(int value) { return (long)value; }
    public static float jitConvertIF(int value) { return (float)value; }
    public static double jitConvertID(int value) { return (double)value; }
    public static int jitConvertJI(long value) { return (int)value; }
    public static float jitConvertJF(long value) { return (float)value; }
    public static double jitConvertJD(long value) { return (double)value; }
    public static int jitConvertFI(float value) { return (int)value; }
    public static long jitConvertFJ(float value) { return (long)value; }
    public static double jitConvertFD(float value) { return (double)value; }
    public static int jitConvertDI(double value) { return (int)value; }
    public static long jitConvertDJ(double value) { return (long)value; }
    public static float jitConvertDF(double value) { return (float)value; }
    public static long jitDivideLong(long a,long b) { return a/b; }
    public static float jitDivideFloat(float a,float b) { return a/b; }
    public static double jitDivideDouble(double a,double b) { return a/b; }
    public static long jitRemainderLong(long a,long b) { return a%b; }
    public static float jitRemainderFloat(float a,float b) { return a%b; }
    public static double jitRemainderDouble(double a,double b) { return a%b; }
    public static long jitNumericLong(long a,long b) { return ((a+b)*(a-b))/(a*b) + a%b; }
    public static float jitNumericFloat(float a,float b) { return ((a+b)*(a-b))/(a*b) + a%b; }
    public static double jitNumericDouble(double a,double b) { return ((a+b)*(a-b))/(a*b) + a%b; }
    public static boolean jitMonitorThrow;
    public static boolean jitMonitorHeld;
    public static void jitMonitorWork(Throwable value) throws Throwable {
        jitMonitorHeld = Thread.holdsLock(value);
        System.gc();
        if (jitMonitorThrow) throw value;
    }
    public static Throwable jitMonitor(Throwable value) throws Throwable {
        synchronized (value) { jitMonitorWork(value); return value; }
    }
    public static int jitFinallyCount;
    public static Throwable jitFinallySeen;
    public static Throwable jitFinallyReplacement;
    public static void jitMaybeThrow(Throwable value) throws Throwable { if (value != null) throw value; }
    public static void jitFinallyMark(Throwable value) throws Throwable {
        System.gc();
        jitFinallyCount++;
        jitFinallySeen = value;
        if (jitFinallyReplacement != null) throw jitFinallyReplacement;
    }
    public static Throwable jitFinally(Throwable value) throws Throwable {
        try { jitMaybeThrow(value); return null; }
        finally { jitFinallyMark(value); }
    }
    public static void jitThrow(Throwable value) throws Throwable { throw value; }
    public static Throwable jitCatchReturn(Throwable value) {
        try { jitThrow(value); return null; }
        catch (Throwable caught) { return caught; }
    }
    public static Throwable jitTypedCatch(Throwable value) throws Throwable {
        try { jitThrow(value); return null; }
        catch (IllegalArgumentException caught) { return caught; }
        catch (IllegalStateException caught) { return null; }
    }
    public static int jitCatchThrow(Throwable value) {
        try { jitThrow(value); return -1; }
        catch (Throwable caught) { return caught == value ? 42 : value == null && caught instanceof NullPointerException ? 41 : -2; }
    }
    public static int jitFinallyCounter;
    public static int jitInterfaceValue(JitCallable receiver) { return receiver.value(); }
    public static long jitInterfaceRangeAa(JitCallable receiver, int a, int b, int c, int d, int e, int f, int g, long wide, double real, Object value) {
        return receiver.Aa(a, b, c, d, e, f, g, wide, real, value);
    }
    public static long jitInterfaceRangeBB(JitCallable receiver, int a, int b, int c, int d, int e, int f, int g, long wide, double real, Object value) {
        return receiver.BB(a, b, c, d, e, f, g, wide, real, value);
    }
    public static int jitInterfaceAa(JitCallable receiver) { return receiver.Aa(); }
    public static int jitInterfaceBB(JitCallable receiver) { return receiver.BB(); }
    public static long jitInterfaceRange(JitCallable receiver, int a, int b, int c, int d, int e, int f, int g, long wide, double real, Object value) {
        return receiver.range(a, b, c, d, e, f, g, wide, real, value);
    }
    public static int jitInterfaceDefault(JitCallable receiver) { return receiver.defaultValue(); }
    public static Object jitInterfaceReference(JitCallable receiver) { return receiver.reference(); }
    public static synchronized Object jitSynchronizedStaticThrow(Throwable value) throws Throwable {
        jitVoidGc();
        throw value;
    }
    public synchronized Object jitSynchronizedInstanceThrow(Throwable value) throws Throwable {
        jitVoidGc();
        throw value;
    }
    public static synchronized Object jitSynchronizedStatic(Object value) {
        jitVoidGc();
        return value;
    }
    public synchronized Object jitSynchronizedInstance(Object value) {
        jitVoidGc();
        return value;
    }
    public static Object jitNestedFinally(Throwable original, Throwable replacement, int mask) throws Throwable {
        try {
            try { throw original; }
            finally {
                jitFinallyCounter = jitFinallyCounter * 10 + 1;
                if ((mask & 1) != 0) throw replacement;
            }
        } finally {
            jitFinallyCounter = jitFinallyCounter * 10 + 2;
            if ((mask & 2) != 0) return replacement;
        }
    }
    public static int jitTryArray(int[] values, int index) {
        try { return values[index]; }
        catch (NullPointerException expected) { return -31; }
        catch (ArrayIndexOutOfBoundsException expected) { return -73; }
        finally { jitFinallyCounter++; }
    }
    public static Hello jitTryConstructor(int marker, Object payload) {
        try { return new Hello(marker, payload); }
        catch (IllegalArgumentException expected) { return null; }
        finally { jitFinallyCounter++; }
    }
    public static Hello jitObjectCompose(int marker, Object payload) {
        Hello first = new Hello(marker, payload);
        Hello second = new Hello();
        second.jitComposedNumber = marker * 31;
        second.jitComposedObject = first;
        jitVoidGc();
        return second;
    }
    public static int[] jitArrayAllocate(int size, int delta) {
        int[] values = new int[size];
        for (int i = 0; i < size; ++i) values[i] = i * 31 + delta;
        return values;
    }
    public static Object[] jitArrayCopyGc(Object[] source) {
        Object[] values = new Object[source.length];
        for (int i = 0; i < source.length; ++i) values[i] = jitGcTarget(source[i]);
        return values;
    }
    public static void jitSystemArrayCopyObject(
            Object source, int sourcePosition, Object destination, int destinationPosition, int length) {
        System.arraycopy(source, sourcePosition, destination, destinationPosition, length);
    }
    public static void jitSystemArrayCopyChar(
            char[] source, int sourcePosition, char[] destination, int destinationPosition, int length) {
        System.arraycopy(source, sourcePosition, destination, destinationPosition, length);
    }
    public static void jitSystemArrayCopyByte(
            byte[] source, int sourcePosition, byte[] destination, int destinationPosition, int length) {
        System.arraycopy(source, sourcePosition, destination, destinationPosition, length);
    }
    public static void jitSystemArrayCopyInt(
            int[] source, int sourcePosition, int[] destination, int destinationPosition, int length) {
        System.arraycopy(source, sourcePosition, destination, destinationPosition, length);
    }
    public static long jitMathHInvoke(
            double x, double y, float a, float b, float c, long left, long right) {
        long result = 17;
        result = result * 31 + Double.doubleToRawLongBits(Math.fma(x, y, x));
        result = result * 31 + Float.floatToRawIntBits(Math.fma(a, b, c));
        result = result * 31 + Double.doubleToRawLongBits(Math.cos(x));
        result = result * 31 + Double.doubleToRawLongBits(Math.sin(x));
        result = result * 31 + Double.doubleToRawLongBits(Math.acos(x));
        result = result * 31 + Double.doubleToRawLongBits(Math.asin(x));
        result = result * 31 + Double.doubleToRawLongBits(Math.atan(x));
        result = result * 31 + Double.doubleToRawLongBits(Math.atan2(x, y));
        result = result * 31 + Double.doubleToRawLongBits(Math.pow(x, y));
        result = result * 31 + Double.doubleToRawLongBits(Math.cbrt(x));
        result = result * 31 + Double.doubleToRawLongBits(Math.cosh(x));
        result = result * 31 + Double.doubleToRawLongBits(Math.exp(x));
        result = result * 31 + Double.doubleToRawLongBits(Math.expm1(x));
        result = result * 31 + Double.doubleToRawLongBits(Math.hypot(x, y));
        result = result * 31 + Double.doubleToRawLongBits(Math.log(x));
        result = result * 31 + Double.doubleToRawLongBits(Math.log10(x));
        result = result * 31 + Double.doubleToRawLongBits(Math.nextAfter(x, y));
        result = result * 31 + Double.doubleToRawLongBits(Math.sinh(x));
        result = result * 31 + Double.doubleToRawLongBits(Math.tan(x));
        result = result * 31 + Double.doubleToRawLongBits(Math.tanh(x));
        result = result * 31 + Double.doubleToRawLongBits(Math.sqrt(x));
        result = result * 31 + Double.doubleToRawLongBits(Math.ceil(x));
        result = result * 31 + Double.doubleToRawLongBits(Math.floor(x));
        result = result * 31 + Double.doubleToRawLongBits(Math.rint(x));
        result = result * 31 + Math.round(x);
        result = result * 31 + Math.round(a);
        result = result * 31 + Double.doubleToRawLongBits(Math.copySign(x, y));
        result = result * 31 + Float.floatToRawIntBits(Math.copySign(a, b));
        return result;
    }
    public static long jitCrc32UpdateInt(int value) {
        java.util.zip.CRC32 crc = new java.util.zip.CRC32();
        crc.update(value);
        return (((long) value) << 32) | (crc.getValue() & 0xffffffffL);
    }
    public static int jitArrayTransform(int[] values, int count, int delta) {
        int sum = 0;
        for (int i = 0; i < count; ++i) {
            values[i] = values[i] * 31 + delta;
            sum += values[i];
        }
        return sum + values.length;
    }
    public int jitComposedNumber;
    public Object jitComposedObject;
    public static volatile long jitComposedWide;
    public int jitFieldUpdate(Hello target, int add) {
        int value = target.jitComposedNumber * 31 + add;
        target.jitComposedNumber = value;
        return target.jitComposedNumber;
    }
    public static long jitWideFieldUpdate(long add) {
        long value = jitComposedWide + add;
        jitComposedWide = value;
        return jitComposedWide;
    }
    public Object jitFieldExchange(Object value) {
        Object old = jitComposedObject;
        jitComposedObject = value;
        return jitGcTarget(old);
    }
    public int jitInstanceMath(int a, int b) {
        return jitArithmetic(a, b) ^ (a < b ? 17 : 93);
    }
    public Object jitInstanceChoose(Object value, int selector) {
        return selector < 0 ? this : value;
    }
    public double jitInstanceWide(long value, double real, int scale) {
        return (double) value + real * scale;
    }
    public static int jitVirtualComposed(JitVirtualBase a, JitVirtualBase b) {
        return a.value() * 31 + b.value();
    }
    public static Object jitVirtualReferences(JitVirtualBase a, JitVirtualBase b) {
        Object first = a.reference();
        Object second = b.reference();
        return first != null ? first : second;
    }
    public static int jitVirtualSecond(JitVirtualBase ignored, JitVirtualBase used) {
        return used.value();
    }
    public static Class<?> jitVirtualBaseClass() { return JitVirtualBase.class; }
    public static Class<?> jitVirtualChildClass() { return JitVirtualChild.class; }
    public static int jitVirtualValue(JitVirtualBase receiver) { return receiver.value(); }
    public static long jitWideSpeculative(JitVirtualBase receiver, int a,int b,int c,int d,int e,int f,int g,long wide,double real,Object value) {
        return receiver.wideSpeculative(a,b,c,d,e,f,g,wide,real,value);
    }
    public static long jitNestedWideSpeculative(JitVirtualBase receiver, int a,int b,int c,int d,int e,int f,int g,long wide,double real,Object value) {
        return jitWideSpeculative(receiver,a,b,c,d,e,f,g,wide,real,value);
    }
    public static long jitVirtualRange(JitVirtualBase receiver, int a, int b, int c, int d, int e, int f, int g, long wide, double real, Object value) {
        return receiver.range(a,b,c,d,e,f,g,wide,real,value);
    }
    public static Object jitVirtualReference(JitVirtualBase receiver) { return receiver.reference(); }
    public static int jitBaselineVirtualValue(JitVirtualBase receiver) { return receiver.value(); }
    public static int jitSpeculativeVirtualValue(JitVirtualBase receiver) { return receiver.value(); }
    public static Object jitSpeculativeVirtualReference(JitVirtualBase receiver) { return receiver.reference(); }
    public static Object jitNestedSpeculativeReference(JitVirtualBase receiver) { return jitSpeculativeVirtualReference(receiver); }
    public static Object jitBaselineVirtualReference(JitVirtualBase receiver) { return receiver.reference(); }
    public static Object jitNewFinalReference(Object payload, int iterations) {
        return new JitFinalReference(payload, iterations);
    }
    public Hello() {}
    public Hello(long wide, double floating) {
        jitInstanceWide = wide;
        jitInstanceFloating = floating;
    }
    public long jitInstanceWide;
    public double jitInstanceFloating;
    public static Hello jitNewWideInstance(long wide, double floating) {
        return new Hello(wide, floating);
    }
    public static Hello jitNewEmptyInstance() { return new Hello(); }
    public Hello(int marker, Object payload) {
        if (marker == 42) System.gc();
        jitSetMarker(marker);
        jitSetPayload(payload);
        if (marker < 0) throw new IllegalArgumentException("constructor marker");
    }
    public int jitInstanceMarker;
    public Object jitInstancePayload;
    public int jitGetMarker() { return jitInstanceMarker; }
    public void jitSetMarker(int marker) { jitInstanceMarker = marker; }
    public Object jitGetPayload() { return jitInstancePayload; }
    public void jitSetPayload(Object payload) { jitInstancePayload = payload; }
    public static Hello jitNewInstance(int marker, Object payload) {
        return new Hello(marker, payload);
    }
    public static native int hostPageSize();
    public static native int nativeUnwindJit();

    public static int jitUnwindBridge() {
        return nativeUnwindJit();
    }
    public static int jitPolymorphicTarget(int value) { return value * 31 + 7; }
    public static java.lang.invoke.MethodHandle jitPolymorphicHandle() throws Exception {
        return java.lang.invoke.MethodHandles.lookup().findStatic(Hello.class, "jitPolymorphicTarget",
                java.lang.invoke.MethodType.methodType(int.class, int.class));
    }
    public static int jitPolymorphicExact(java.lang.invoke.MethodHandle handle, int value) throws Throwable {
        return (int) handle.invokeExact(value);
    }
    public static java.lang.invoke.MethodHandle jitPolymorphicReferenceHandle() throws Exception {
        return java.lang.invoke.MethodHandles.lookup().findStatic(Hello.class, "jitGcTarget",
                java.lang.invoke.MethodType.methodType(Object.class, Object.class));
    }
    public static Object jitPolymorphicReference(java.lang.invoke.MethodHandle handle, Object value) throws Throwable {
        return (Object) handle.invokeExact(value);
    }
    public static Object jitPolymorphicAdaptedTarget(Hello value) {
        System.gc();
        return value;
    }
    public static java.lang.invoke.MethodHandle jitPolymorphicAdaptedHandle() throws Exception {
        return java.lang.invoke.MethodHandles.lookup().findStatic(Hello.class, "jitPolymorphicAdaptedTarget",
                java.lang.invoke.MethodType.methodType(Object.class, Hello.class)).asType(
                java.lang.invoke.MethodType.methodType(Object.class, Object.class));
    }
    public static Object jitPolymorphicAdapted(java.lang.invoke.MethodHandle handle, Object value) throws Throwable {
        return (Object) handle.invokeExact(value);
    }
    public static long jitPolymorphicInexact(java.lang.invoke.MethodHandle handle, int value) throws Throwable {
        return (long) handle.invoke(value);
    }
    public Object jitHandleReference;
    public int jitVarValue;
    public static int jitVarStaticValue;
    public int jitVarPayload;
    public Object jitVarReference;
    public static Object jitVarStaticReference;
    public long jitVarLong;
    public static long jitVarStaticLong;
    public double jitVarDouble;
    public static double jitVarStaticDouble;
    public float jitVarFloat;
    public static float jitVarStaticFloat;
    public byte jitVarByte;
    public static byte jitVarStaticByte;
    public boolean jitVarBoolean;
    public static boolean jitVarStaticBoolean;
    public short jitVarShort;
    public static short jitVarStaticShort;
    public char jitVarChar;
    public static char jitVarStaticChar;
    public static java.lang.invoke.VarHandle jitVarLongHandle(boolean stat) throws Exception {
        String method = stat ? "findStaticVarHandle" : "findVarHandle";
        return (java.lang.invoke.VarHandle) java.lang.invoke.MethodHandles.Lookup.class
                .getMethod(method, Class.class, String.class, Class.class)
                .invoke(java.lang.invoke.MethodHandles.lookup(), Hello.class,
                        stat ? "jitVarStaticLong" : "jitVarLong", long.class);
    }
    public static long jitVarLongGet(java.lang.invoke.VarHandle handle, Hello receiver) {
        return (long) handle.get(receiver);
    }
    public static void jitVarLongSet(java.lang.invoke.VarHandle handle, Hello receiver, long value) {
        handle.set(receiver, value);
    }
    public static boolean jitVarLongCas(java.lang.invoke.VarHandle handle, Hello receiver, long expected, long value) {
        return handle.compareAndSet(receiver, expected, value);
    }
    public static long jitVarLongAdd(java.lang.invoke.VarHandle handle, Hello receiver, long value) {
        return (long) handle.getAndAdd(receiver, value);
    }
    public static long jitVarStaticLongGet(java.lang.invoke.VarHandle handle) {
        return (long) handle.get();
    }
    public static void jitVarStaticLongSet(java.lang.invoke.VarHandle handle, long value) {
        handle.set(value);
    }
    public static boolean jitVarStaticLongCas(java.lang.invoke.VarHandle handle, long expected, long value) {
        return handle.compareAndSet(expected, value);
    }
    public static long jitVarStaticLongAdd(java.lang.invoke.VarHandle handle, long value) {
        return (long) handle.getAndAdd(value);
    }
    public static long jitVarLongGetOpaque(java.lang.invoke.VarHandle h, Hello r) { return (long) h.getOpaque(r); }
    public static long jitVarLongGetAcquire(java.lang.invoke.VarHandle h, Hello r) { return (long) h.getAcquire(r); }
    public static long jitVarLongGetVolatile(java.lang.invoke.VarHandle h, Hello r) { return (long) h.getVolatile(r); }
    public static void jitVarLongSetOpaque(java.lang.invoke.VarHandle h, Hello r, long v) { h.setOpaque(r, v); }
    public static void jitVarLongSetRelease(java.lang.invoke.VarHandle h, Hello r, long v) { h.setRelease(r, v); }
    public static void jitVarLongSetVolatile(java.lang.invoke.VarHandle h, Hello r, long v) { h.setVolatile(r, v); }
    public static boolean jitVarLongWeakPlain(java.lang.invoke.VarHandle h, Hello r, long e, long v) { return h.weakCompareAndSetPlain(r, e, v); }
    public static boolean jitVarLongWeakAcquire(java.lang.invoke.VarHandle h, Hello r, long e, long v) { return h.weakCompareAndSetAcquire(r, e, v); }
    public static boolean jitVarLongWeakRelease(java.lang.invoke.VarHandle h, Hello r, long e, long v) { return h.weakCompareAndSetRelease(r, e, v); }
    public static boolean jitVarLongWeakVolatile(java.lang.invoke.VarHandle h, Hello r, long e, long v) { return h.weakCompareAndSet(r, e, v); }
    public static long jitVarLongExchangeAcquire(java.lang.invoke.VarHandle h, Hello r, long e, long v) { return (long) h.compareAndExchangeAcquire(r, e, v); }
    public static long jitVarLongExchangeRelease(java.lang.invoke.VarHandle h, Hello r, long e, long v) { return (long) h.compareAndExchangeRelease(r, e, v); }
    public static long jitVarLongSwapAcquire(java.lang.invoke.VarHandle h, Hello r, long v) { return (long) h.getAndSetAcquire(r, v); }
    public static long jitVarLongSwapRelease(java.lang.invoke.VarHandle h, Hello r, long v) { return (long) h.getAndSetRelease(r, v); }
    public static long jitVarLongAddAcquire(java.lang.invoke.VarHandle h, Hello r, long v) { return (long) h.getAndAddAcquire(r, v); }
    public static long jitVarLongAddRelease(java.lang.invoke.VarHandle h, Hello r, long v) { return (long) h.getAndAddRelease(r, v); }
    public static long jitVarLongOr(java.lang.invoke.VarHandle h, Hello r, long v) { return (long) h.getAndBitwiseOr(r, v); }
    public static long jitVarLongOrAcquire(java.lang.invoke.VarHandle h, Hello r, long v) { return (long) h.getAndBitwiseOrAcquire(r, v); }
    public static long jitVarLongOrRelease(java.lang.invoke.VarHandle h, Hello r, long v) { return (long) h.getAndBitwiseOrRelease(r, v); }
    public static long jitVarLongAnd(java.lang.invoke.VarHandle h, Hello r, long v) { return (long) h.getAndBitwiseAnd(r, v); }
    public static long jitVarLongAndAcquire(java.lang.invoke.VarHandle h, Hello r, long v) { return (long) h.getAndBitwiseAndAcquire(r, v); }
    public static long jitVarLongAndRelease(java.lang.invoke.VarHandle h, Hello r, long v) { return (long) h.getAndBitwiseAndRelease(r, v); }
    public static long jitVarLongXorAcquire(java.lang.invoke.VarHandle h, Hello r, long v) { return (long) h.getAndBitwiseXorAcquire(r, v); }
    public static long jitVarLongXorRelease(java.lang.invoke.VarHandle h, Hello r, long v) { return (long) h.getAndBitwiseXorRelease(r, v); }
    public static long jitVarStaticLongGetOpaque(java.lang.invoke.VarHandle h) { return (long) h.getOpaque(); }
    public static long jitVarStaticLongGetAcquire(java.lang.invoke.VarHandle h) { return (long) h.getAcquire(); }
    public static long jitVarStaticLongGetVolatile(java.lang.invoke.VarHandle h) { return (long) h.getVolatile(); }
    public static void jitVarStaticLongSetOpaque(java.lang.invoke.VarHandle h, long v) { h.setOpaque(v); }
    public static void jitVarStaticLongSetRelease(java.lang.invoke.VarHandle h, long v) { h.setRelease(v); }
    public static void jitVarStaticLongSetVolatile(java.lang.invoke.VarHandle h, long v) { h.setVolatile(v); }
    public static boolean jitVarStaticLongWeakPlain(java.lang.invoke.VarHandle h, long e, long v) { return h.weakCompareAndSetPlain(e, v); }
    public static boolean jitVarStaticLongWeakAcquire(java.lang.invoke.VarHandle h, long e, long v) { return h.weakCompareAndSetAcquire(e, v); }
    public static boolean jitVarStaticLongWeakRelease(java.lang.invoke.VarHandle h, long e, long v) { return h.weakCompareAndSetRelease(e, v); }
    public static boolean jitVarStaticLongWeakVolatile(java.lang.invoke.VarHandle h, long e, long v) { return h.weakCompareAndSet(e, v); }
    public static long jitVarStaticLongExchangeAcquire(java.lang.invoke.VarHandle h, long e, long v) { return (long) h.compareAndExchangeAcquire(e, v); }
    public static long jitVarStaticLongExchangeRelease(java.lang.invoke.VarHandle h, long e, long v) { return (long) h.compareAndExchangeRelease(e, v); }
    public static long jitVarStaticLongSwapAcquire(java.lang.invoke.VarHandle h, long v) { return (long) h.getAndSetAcquire(v); }
    public static long jitVarStaticLongSwapRelease(java.lang.invoke.VarHandle h, long v) { return (long) h.getAndSetRelease(v); }
    public static long jitVarStaticLongAddAcquire(java.lang.invoke.VarHandle h, long v) { return (long) h.getAndAddAcquire(v); }
    public static long jitVarStaticLongAddRelease(java.lang.invoke.VarHandle h, long v) { return (long) h.getAndAddRelease(v); }
    public static long jitVarStaticLongOr(java.lang.invoke.VarHandle h, long v) { return (long) h.getAndBitwiseOr(v); }
    public static long jitVarStaticLongOrAcquire(java.lang.invoke.VarHandle h, long v) { return (long) h.getAndBitwiseOrAcquire(v); }
    public static long jitVarStaticLongOrRelease(java.lang.invoke.VarHandle h, long v) { return (long) h.getAndBitwiseOrRelease(v); }
    public static long jitVarStaticLongAnd(java.lang.invoke.VarHandle h, long v) { return (long) h.getAndBitwiseAnd(v); }
    public static long jitVarStaticLongAndAcquire(java.lang.invoke.VarHandle h, long v) { return (long) h.getAndBitwiseAndAcquire(v); }
    public static long jitVarStaticLongAndRelease(java.lang.invoke.VarHandle h, long v) { return (long) h.getAndBitwiseAndRelease(v); }
    public static long jitVarStaticLongXorAcquire(java.lang.invoke.VarHandle h, long v) { return (long) h.getAndBitwiseXorAcquire(v); }
    public static long jitVarStaticLongXorRelease(java.lang.invoke.VarHandle h, long v) { return (long) h.getAndBitwiseXorRelease(v); }
    public static long jitVarLongArrayGetOpaque(java.lang.invoke.VarHandle h, long[] a, int i) { return (long) h.getOpaque(a, i); }
    public static long jitVarLongArrayGetAcquire(java.lang.invoke.VarHandle h, long[] a, int i) { return (long) h.getAcquire(a, i); }
    public static long jitVarLongArrayGetVolatile(java.lang.invoke.VarHandle h, long[] a, int i) { return (long) h.getVolatile(a, i); }
    public static void jitVarLongArraySetOpaque(java.lang.invoke.VarHandle h, long[] a, int i, long v) { h.setOpaque(a, i, v); }
    public static void jitVarLongArraySetRelease(java.lang.invoke.VarHandle h, long[] a, int i, long v) { h.setRelease(a, i, v); }
    public static void jitVarLongArraySetVolatile(java.lang.invoke.VarHandle h, long[] a, int i, long v) { h.setVolatile(a, i, v); }
    public static boolean jitVarLongArrayWeakPlain(java.lang.invoke.VarHandle h, long[] a, int i, long e, long v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarLongArrayWeakAcquire(java.lang.invoke.VarHandle h, long[] a, int i, long e, long v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarLongArrayWeakRelease(java.lang.invoke.VarHandle h, long[] a, int i, long e, long v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarLongArrayWeakVolatile(java.lang.invoke.VarHandle h, long[] a, int i, long e, long v) { return h.weakCompareAndSet(a, i, e, v); }
    public static long jitVarLongArrayExchangeAcquire(java.lang.invoke.VarHandle h, long[] a, int i, long e, long v) { return (long) h.compareAndExchangeAcquire(a, i, e, v); }
    public static long jitVarLongArrayExchangeRelease(java.lang.invoke.VarHandle h, long[] a, int i, long e, long v) { return (long) h.compareAndExchangeRelease(a, i, e, v); }
    public static long jitVarLongArraySwapAcquire(java.lang.invoke.VarHandle h, long[] a, int i, long v) { return (long) h.getAndSetAcquire(a, i, v); }
    public static long jitVarLongArraySwapRelease(java.lang.invoke.VarHandle h, long[] a, int i, long v) { return (long) h.getAndSetRelease(a, i, v); }
    public static long jitVarLongArrayAddAcquire(java.lang.invoke.VarHandle h, long[] a, int i, long v) { return (long) h.getAndAddAcquire(a, i, v); }
    public static long jitVarLongArrayAddRelease(java.lang.invoke.VarHandle h, long[] a, int i, long v) { return (long) h.getAndAddRelease(a, i, v); }
    public static long jitVarLongArrayOr(java.lang.invoke.VarHandle h, long[] a, int i, long v) { return (long) h.getAndBitwiseOr(a, i, v); }
    public static long jitVarLongArrayOrAcquire(java.lang.invoke.VarHandle h, long[] a, int i, long v) { return (long) h.getAndBitwiseOrAcquire(a, i, v); }
    public static long jitVarLongArrayOrRelease(java.lang.invoke.VarHandle h, long[] a, int i, long v) { return (long) h.getAndBitwiseOrRelease(a, i, v); }
    public static long jitVarLongArrayAnd(java.lang.invoke.VarHandle h, long[] a, int i, long v) { return (long) h.getAndBitwiseAnd(a, i, v); }
    public static long jitVarLongArrayAndAcquire(java.lang.invoke.VarHandle h, long[] a, int i, long v) { return (long) h.getAndBitwiseAndAcquire(a, i, v); }
    public static long jitVarLongArrayAndRelease(java.lang.invoke.VarHandle h, long[] a, int i, long v) { return (long) h.getAndBitwiseAndRelease(a, i, v); }
    public static long jitVarLongArrayXorAcquire(java.lang.invoke.VarHandle h, long[] a, int i, long v) { return (long) h.getAndBitwiseXorAcquire(a, i, v); }
    public static long jitVarLongArrayXorRelease(java.lang.invoke.VarHandle h, long[] a, int i, long v) { return (long) h.getAndBitwiseXorRelease(a, i, v); }
    public static float jitVarFloatGetOpaque(java.lang.invoke.VarHandle h, Hello r) { return (float) h.getOpaque(r); }
    public static float jitVarFloatGetAcquire(java.lang.invoke.VarHandle h, Hello r) { return (float) h.getAcquire(r); }
    public static float jitVarFloatGetVolatile(java.lang.invoke.VarHandle h, Hello r) { return (float) h.getVolatile(r); }
    public static void jitVarFloatSetOpaque(java.lang.invoke.VarHandle h, Hello r, float v) { h.setOpaque(r, v); }
    public static void jitVarFloatSetRelease(java.lang.invoke.VarHandle h, Hello r, float v) { h.setRelease(r, v); }
    public static void jitVarFloatSetVolatile(java.lang.invoke.VarHandle h, Hello r, float v) { h.setVolatile(r, v); }
    public static boolean jitVarFloatWeakPlain(java.lang.invoke.VarHandle h, Hello r, float e, float v) { return h.weakCompareAndSetPlain(r, e, v); }
    public static boolean jitVarFloatWeakAcquire(java.lang.invoke.VarHandle h, Hello r, float e, float v) { return h.weakCompareAndSetAcquire(r, e, v); }
    public static boolean jitVarFloatWeakRelease(java.lang.invoke.VarHandle h, Hello r, float e, float v) { return h.weakCompareAndSetRelease(r, e, v); }
    public static boolean jitVarFloatWeakVolatile(java.lang.invoke.VarHandle h, Hello r, float e, float v) { return h.weakCompareAndSet(r, e, v); }
    public static float jitVarFloatExchangeAcquire(java.lang.invoke.VarHandle h, Hello r, float e, float v) { return (float) h.compareAndExchangeAcquire(r, e, v); }
    public static float jitVarFloatExchangeRelease(java.lang.invoke.VarHandle h, Hello r, float e, float v) { return (float) h.compareAndExchangeRelease(r, e, v); }
    public static float jitVarFloatSwapAcquire(java.lang.invoke.VarHandle h, Hello r, float v) { return (float) h.getAndSetAcquire(r, v); }
    public static float jitVarFloatSwapRelease(java.lang.invoke.VarHandle h, Hello r, float v) { return (float) h.getAndSetRelease(r, v); }
    public static float jitVarFloatAddAcquire(java.lang.invoke.VarHandle h, Hello r, float v) { return (float) h.getAndAddAcquire(r, v); }
    public static float jitVarFloatAddRelease(java.lang.invoke.VarHandle h, Hello r, float v) { return (float) h.getAndAddRelease(r, v); }
    public static float jitVarFloatOr(java.lang.invoke.VarHandle h, Hello r, float v) { return (float) h.getAndBitwiseOr(r, v); }
    public static float jitVarFloatOrAcquire(java.lang.invoke.VarHandle h, Hello r, float v) { return (float) h.getAndBitwiseOrAcquire(r, v); }
    public static float jitVarFloatOrRelease(java.lang.invoke.VarHandle h, Hello r, float v) { return (float) h.getAndBitwiseOrRelease(r, v); }
    public static float jitVarFloatAnd(java.lang.invoke.VarHandle h, Hello r, float v) { return (float) h.getAndBitwiseAnd(r, v); }
    public static float jitVarFloatAndAcquire(java.lang.invoke.VarHandle h, Hello r, float v) { return (float) h.getAndBitwiseAndAcquire(r, v); }
    public static float jitVarFloatAndRelease(java.lang.invoke.VarHandle h, Hello r, float v) { return (float) h.getAndBitwiseAndRelease(r, v); }
    public static float jitVarFloatXorAcquire(java.lang.invoke.VarHandle h, Hello r, float v) { return (float) h.getAndBitwiseXorAcquire(r, v); }
    public static float jitVarFloatXorRelease(java.lang.invoke.VarHandle h, Hello r, float v) { return (float) h.getAndBitwiseXorRelease(r, v); }
    public static float jitVarStaticFloatGetOpaque(java.lang.invoke.VarHandle h) { return (float) h.getOpaque(); }
    public static float jitVarStaticFloatGetAcquire(java.lang.invoke.VarHandle h) { return (float) h.getAcquire(); }
    public static float jitVarStaticFloatGetVolatile(java.lang.invoke.VarHandle h) { return (float) h.getVolatile(); }
    public static void jitVarStaticFloatSetOpaque(java.lang.invoke.VarHandle h, float v) { h.setOpaque(v); }
    public static void jitVarStaticFloatSetRelease(java.lang.invoke.VarHandle h, float v) { h.setRelease(v); }
    public static void jitVarStaticFloatSetVolatile(java.lang.invoke.VarHandle h, float v) { h.setVolatile(v); }
    public static boolean jitVarStaticFloatWeakPlain(java.lang.invoke.VarHandle h, float e, float v) { return h.weakCompareAndSetPlain(e, v); }
    public static boolean jitVarStaticFloatWeakAcquire(java.lang.invoke.VarHandle h, float e, float v) { return h.weakCompareAndSetAcquire(e, v); }
    public static boolean jitVarStaticFloatWeakRelease(java.lang.invoke.VarHandle h, float e, float v) { return h.weakCompareAndSetRelease(e, v); }
    public static boolean jitVarStaticFloatWeakVolatile(java.lang.invoke.VarHandle h, float e, float v) { return h.weakCompareAndSet(e, v); }
    public static float jitVarStaticFloatExchangeAcquire(java.lang.invoke.VarHandle h, float e, float v) { return (float) h.compareAndExchangeAcquire(e, v); }
    public static float jitVarStaticFloatExchangeRelease(java.lang.invoke.VarHandle h, float e, float v) { return (float) h.compareAndExchangeRelease(e, v); }
    public static float jitVarStaticFloatSwapAcquire(java.lang.invoke.VarHandle h, float v) { return (float) h.getAndSetAcquire(v); }
    public static float jitVarStaticFloatSwapRelease(java.lang.invoke.VarHandle h, float v) { return (float) h.getAndSetRelease(v); }
    public static float jitVarStaticFloatAddAcquire(java.lang.invoke.VarHandle h, float v) { return (float) h.getAndAddAcquire(v); }
    public static float jitVarStaticFloatAddRelease(java.lang.invoke.VarHandle h, float v) { return (float) h.getAndAddRelease(v); }
    public static float jitVarStaticFloatOr(java.lang.invoke.VarHandle h, float v) { return (float) h.getAndBitwiseOr(v); }
    public static float jitVarStaticFloatOrAcquire(java.lang.invoke.VarHandle h, float v) { return (float) h.getAndBitwiseOrAcquire(v); }
    public static float jitVarStaticFloatOrRelease(java.lang.invoke.VarHandle h, float v) { return (float) h.getAndBitwiseOrRelease(v); }
    public static float jitVarStaticFloatAnd(java.lang.invoke.VarHandle h, float v) { return (float) h.getAndBitwiseAnd(v); }
    public static float jitVarStaticFloatAndAcquire(java.lang.invoke.VarHandle h, float v) { return (float) h.getAndBitwiseAndAcquire(v); }
    public static float jitVarStaticFloatAndRelease(java.lang.invoke.VarHandle h, float v) { return (float) h.getAndBitwiseAndRelease(v); }
    public static float jitVarStaticFloatXorAcquire(java.lang.invoke.VarHandle h, float v) { return (float) h.getAndBitwiseXorAcquire(v); }
    public static float jitVarStaticFloatXorRelease(java.lang.invoke.VarHandle h, float v) { return (float) h.getAndBitwiseXorRelease(v); }
    public static float jitVarFloatArrayGetOpaque(java.lang.invoke.VarHandle h, float[] a, int i) { return (float) h.getOpaque(a, i); }
    public static float jitVarFloatArrayGetAcquire(java.lang.invoke.VarHandle h, float[] a, int i) { return (float) h.getAcquire(a, i); }
    public static float jitVarFloatArrayGetVolatile(java.lang.invoke.VarHandle h, float[] a, int i) { return (float) h.getVolatile(a, i); }
    public static void jitVarFloatArraySetOpaque(java.lang.invoke.VarHandle h, float[] a, int i, float v) { h.setOpaque(a, i, v); }
    public static void jitVarFloatArraySetRelease(java.lang.invoke.VarHandle h, float[] a, int i, float v) { h.setRelease(a, i, v); }
    public static void jitVarFloatArraySetVolatile(java.lang.invoke.VarHandle h, float[] a, int i, float v) { h.setVolatile(a, i, v); }
    public static boolean jitVarFloatArrayWeakPlain(java.lang.invoke.VarHandle h, float[] a, int i, float e, float v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarFloatArrayWeakAcquire(java.lang.invoke.VarHandle h, float[] a, int i, float e, float v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarFloatArrayWeakRelease(java.lang.invoke.VarHandle h, float[] a, int i, float e, float v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarFloatArrayWeakVolatile(java.lang.invoke.VarHandle h, float[] a, int i, float e, float v) { return h.weakCompareAndSet(a, i, e, v); }
    public static float jitVarFloatArrayExchangeAcquire(java.lang.invoke.VarHandle h, float[] a, int i, float e, float v) { return (float) h.compareAndExchangeAcquire(a, i, e, v); }
    public static float jitVarFloatArrayExchangeRelease(java.lang.invoke.VarHandle h, float[] a, int i, float e, float v) { return (float) h.compareAndExchangeRelease(a, i, e, v); }
    public static float jitVarFloatArraySwapAcquire(java.lang.invoke.VarHandle h, float[] a, int i, float v) { return (float) h.getAndSetAcquire(a, i, v); }
    public static float jitVarFloatArraySwapRelease(java.lang.invoke.VarHandle h, float[] a, int i, float v) { return (float) h.getAndSetRelease(a, i, v); }
    public static float jitVarFloatArrayAddAcquire(java.lang.invoke.VarHandle h, float[] a, int i, float v) { return (float) h.getAndAddAcquire(a, i, v); }
    public static float jitVarFloatArrayAddRelease(java.lang.invoke.VarHandle h, float[] a, int i, float v) { return (float) h.getAndAddRelease(a, i, v); }
    public static float jitVarFloatArrayOr(java.lang.invoke.VarHandle h, float[] a, int i, float v) { return (float) h.getAndBitwiseOr(a, i, v); }
    public static float jitVarFloatArrayOrAcquire(java.lang.invoke.VarHandle h, float[] a, int i, float v) { return (float) h.getAndBitwiseOrAcquire(a, i, v); }
    public static float jitVarFloatArrayOrRelease(java.lang.invoke.VarHandle h, float[] a, int i, float v) { return (float) h.getAndBitwiseOrRelease(a, i, v); }
    public static float jitVarFloatArrayAnd(java.lang.invoke.VarHandle h, float[] a, int i, float v) { return (float) h.getAndBitwiseAnd(a, i, v); }
    public static float jitVarFloatArrayAndAcquire(java.lang.invoke.VarHandle h, float[] a, int i, float v) { return (float) h.getAndBitwiseAndAcquire(a, i, v); }
    public static float jitVarFloatArrayAndRelease(java.lang.invoke.VarHandle h, float[] a, int i, float v) { return (float) h.getAndBitwiseAndRelease(a, i, v); }
    public static float jitVarFloatArrayXorAcquire(java.lang.invoke.VarHandle h, float[] a, int i, float v) { return (float) h.getAndBitwiseXorAcquire(a, i, v); }
    public static float jitVarFloatArrayXorRelease(java.lang.invoke.VarHandle h, float[] a, int i, float v) { return (float) h.getAndBitwiseXorRelease(a, i, v); }
    public static double jitVarDoubleGetOpaque(java.lang.invoke.VarHandle h, Hello r) { return (double) h.getOpaque(r); }
    public static double jitVarDoubleGetAcquire(java.lang.invoke.VarHandle h, Hello r) { return (double) h.getAcquire(r); }
    public static double jitVarDoubleGetVolatile(java.lang.invoke.VarHandle h, Hello r) { return (double) h.getVolatile(r); }
    public static void jitVarDoubleSetOpaque(java.lang.invoke.VarHandle h, Hello r, double v) { h.setOpaque(r, v); }
    public static void jitVarDoubleSetRelease(java.lang.invoke.VarHandle h, Hello r, double v) { h.setRelease(r, v); }
    public static void jitVarDoubleSetVolatile(java.lang.invoke.VarHandle h, Hello r, double v) { h.setVolatile(r, v); }
    public static boolean jitVarDoubleWeakPlain(java.lang.invoke.VarHandle h, Hello r, double e, double v) { return h.weakCompareAndSetPlain(r, e, v); }
    public static boolean jitVarDoubleWeakAcquire(java.lang.invoke.VarHandle h, Hello r, double e, double v) { return h.weakCompareAndSetAcquire(r, e, v); }
    public static boolean jitVarDoubleWeakRelease(java.lang.invoke.VarHandle h, Hello r, double e, double v) { return h.weakCompareAndSetRelease(r, e, v); }
    public static boolean jitVarDoubleWeakVolatile(java.lang.invoke.VarHandle h, Hello r, double e, double v) { return h.weakCompareAndSet(r, e, v); }
    public static double jitVarDoubleExchangeAcquire(java.lang.invoke.VarHandle h, Hello r, double e, double v) { return (double) h.compareAndExchangeAcquire(r, e, v); }
    public static double jitVarDoubleExchangeRelease(java.lang.invoke.VarHandle h, Hello r, double e, double v) { return (double) h.compareAndExchangeRelease(r, e, v); }
    public static double jitVarDoubleSwapAcquire(java.lang.invoke.VarHandle h, Hello r, double v) { return (double) h.getAndSetAcquire(r, v); }
    public static double jitVarDoubleSwapRelease(java.lang.invoke.VarHandle h, Hello r, double v) { return (double) h.getAndSetRelease(r, v); }
    public static double jitVarDoubleAddAcquire(java.lang.invoke.VarHandle h, Hello r, double v) { return (double) h.getAndAddAcquire(r, v); }
    public static double jitVarDoubleAddRelease(java.lang.invoke.VarHandle h, Hello r, double v) { return (double) h.getAndAddRelease(r, v); }
    public static double jitVarDoubleOr(java.lang.invoke.VarHandle h, Hello r, double v) { return (double) h.getAndBitwiseOr(r, v); }
    public static double jitVarDoubleOrAcquire(java.lang.invoke.VarHandle h, Hello r, double v) { return (double) h.getAndBitwiseOrAcquire(r, v); }
    public static double jitVarDoubleOrRelease(java.lang.invoke.VarHandle h, Hello r, double v) { return (double) h.getAndBitwiseOrRelease(r, v); }
    public static double jitVarDoubleAnd(java.lang.invoke.VarHandle h, Hello r, double v) { return (double) h.getAndBitwiseAnd(r, v); }
    public static double jitVarDoubleAndAcquire(java.lang.invoke.VarHandle h, Hello r, double v) { return (double) h.getAndBitwiseAndAcquire(r, v); }
    public static double jitVarDoubleAndRelease(java.lang.invoke.VarHandle h, Hello r, double v) { return (double) h.getAndBitwiseAndRelease(r, v); }
    public static double jitVarDoubleXorAcquire(java.lang.invoke.VarHandle h, Hello r, double v) { return (double) h.getAndBitwiseXorAcquire(r, v); }
    public static double jitVarDoubleXorRelease(java.lang.invoke.VarHandle h, Hello r, double v) { return (double) h.getAndBitwiseXorRelease(r, v); }
    public static double jitVarStaticDoubleGetOpaque(java.lang.invoke.VarHandle h) { return (double) h.getOpaque(); }
    public static double jitVarStaticDoubleGetAcquire(java.lang.invoke.VarHandle h) { return (double) h.getAcquire(); }
    public static double jitVarStaticDoubleGetVolatile(java.lang.invoke.VarHandle h) { return (double) h.getVolatile(); }
    public static void jitVarStaticDoubleSetOpaque(java.lang.invoke.VarHandle h, double v) { h.setOpaque(v); }
    public static void jitVarStaticDoubleSetRelease(java.lang.invoke.VarHandle h, double v) { h.setRelease(v); }
    public static void jitVarStaticDoubleSetVolatile(java.lang.invoke.VarHandle h, double v) { h.setVolatile(v); }
    public static boolean jitVarStaticDoubleWeakPlain(java.lang.invoke.VarHandle h, double e, double v) { return h.weakCompareAndSetPlain(e, v); }
    public static boolean jitVarStaticDoubleWeakAcquire(java.lang.invoke.VarHandle h, double e, double v) { return h.weakCompareAndSetAcquire(e, v); }
    public static boolean jitVarStaticDoubleWeakRelease(java.lang.invoke.VarHandle h, double e, double v) { return h.weakCompareAndSetRelease(e, v); }
    public static boolean jitVarStaticDoubleWeakVolatile(java.lang.invoke.VarHandle h, double e, double v) { return h.weakCompareAndSet(e, v); }
    public static double jitVarStaticDoubleExchangeAcquire(java.lang.invoke.VarHandle h, double e, double v) { return (double) h.compareAndExchangeAcquire(e, v); }
    public static double jitVarStaticDoubleExchangeRelease(java.lang.invoke.VarHandle h, double e, double v) { return (double) h.compareAndExchangeRelease(e, v); }
    public static double jitVarStaticDoubleSwapAcquire(java.lang.invoke.VarHandle h, double v) { return (double) h.getAndSetAcquire(v); }
    public static double jitVarStaticDoubleSwapRelease(java.lang.invoke.VarHandle h, double v) { return (double) h.getAndSetRelease(v); }
    public static double jitVarStaticDoubleAddAcquire(java.lang.invoke.VarHandle h, double v) { return (double) h.getAndAddAcquire(v); }
    public static double jitVarStaticDoubleAddRelease(java.lang.invoke.VarHandle h, double v) { return (double) h.getAndAddRelease(v); }
    public static double jitVarStaticDoubleOr(java.lang.invoke.VarHandle h, double v) { return (double) h.getAndBitwiseOr(v); }
    public static double jitVarStaticDoubleOrAcquire(java.lang.invoke.VarHandle h, double v) { return (double) h.getAndBitwiseOrAcquire(v); }
    public static double jitVarStaticDoubleOrRelease(java.lang.invoke.VarHandle h, double v) { return (double) h.getAndBitwiseOrRelease(v); }
    public static double jitVarStaticDoubleAnd(java.lang.invoke.VarHandle h, double v) { return (double) h.getAndBitwiseAnd(v); }
    public static double jitVarStaticDoubleAndAcquire(java.lang.invoke.VarHandle h, double v) { return (double) h.getAndBitwiseAndAcquire(v); }
    public static double jitVarStaticDoubleAndRelease(java.lang.invoke.VarHandle h, double v) { return (double) h.getAndBitwiseAndRelease(v); }
    public static double jitVarStaticDoubleXorAcquire(java.lang.invoke.VarHandle h, double v) { return (double) h.getAndBitwiseXorAcquire(v); }
    public static double jitVarStaticDoubleXorRelease(java.lang.invoke.VarHandle h, double v) { return (double) h.getAndBitwiseXorRelease(v); }
    public static double jitVarDoubleArrayGetOpaque(java.lang.invoke.VarHandle h, double[] a, int i) { return (double) h.getOpaque(a, i); }
    public static double jitVarDoubleArrayGetAcquire(java.lang.invoke.VarHandle h, double[] a, int i) { return (double) h.getAcquire(a, i); }
    public static double jitVarDoubleArrayGetVolatile(java.lang.invoke.VarHandle h, double[] a, int i) { return (double) h.getVolatile(a, i); }
    public static void jitVarDoubleArraySetOpaque(java.lang.invoke.VarHandle h, double[] a, int i, double v) { h.setOpaque(a, i, v); }
    public static void jitVarDoubleArraySetRelease(java.lang.invoke.VarHandle h, double[] a, int i, double v) { h.setRelease(a, i, v); }
    public static void jitVarDoubleArraySetVolatile(java.lang.invoke.VarHandle h, double[] a, int i, double v) { h.setVolatile(a, i, v); }
    public static boolean jitVarDoubleArrayWeakPlain(java.lang.invoke.VarHandle h, double[] a, int i, double e, double v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarDoubleArrayWeakAcquire(java.lang.invoke.VarHandle h, double[] a, int i, double e, double v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarDoubleArrayWeakRelease(java.lang.invoke.VarHandle h, double[] a, int i, double e, double v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarDoubleArrayWeakVolatile(java.lang.invoke.VarHandle h, double[] a, int i, double e, double v) { return h.weakCompareAndSet(a, i, e, v); }
    public static double jitVarDoubleArrayExchangeAcquire(java.lang.invoke.VarHandle h, double[] a, int i, double e, double v) { return (double) h.compareAndExchangeAcquire(a, i, e, v); }
    public static double jitVarDoubleArrayExchangeRelease(java.lang.invoke.VarHandle h, double[] a, int i, double e, double v) { return (double) h.compareAndExchangeRelease(a, i, e, v); }
    public static double jitVarDoubleArraySwapAcquire(java.lang.invoke.VarHandle h, double[] a, int i, double v) { return (double) h.getAndSetAcquire(a, i, v); }
    public static double jitVarDoubleArraySwapRelease(java.lang.invoke.VarHandle h, double[] a, int i, double v) { return (double) h.getAndSetRelease(a, i, v); }
    public static double jitVarDoubleArrayAddAcquire(java.lang.invoke.VarHandle h, double[] a, int i, double v) { return (double) h.getAndAddAcquire(a, i, v); }
    public static double jitVarDoubleArrayAddRelease(java.lang.invoke.VarHandle h, double[] a, int i, double v) { return (double) h.getAndAddRelease(a, i, v); }
    public static double jitVarDoubleArrayOr(java.lang.invoke.VarHandle h, double[] a, int i, double v) { return (double) h.getAndBitwiseOr(a, i, v); }
    public static double jitVarDoubleArrayOrAcquire(java.lang.invoke.VarHandle h, double[] a, int i, double v) { return (double) h.getAndBitwiseOrAcquire(a, i, v); }
    public static double jitVarDoubleArrayOrRelease(java.lang.invoke.VarHandle h, double[] a, int i, double v) { return (double) h.getAndBitwiseOrRelease(a, i, v); }
    public static double jitVarDoubleArrayAnd(java.lang.invoke.VarHandle h, double[] a, int i, double v) { return (double) h.getAndBitwiseAnd(a, i, v); }
    public static double jitVarDoubleArrayAndAcquire(java.lang.invoke.VarHandle h, double[] a, int i, double v) { return (double) h.getAndBitwiseAndAcquire(a, i, v); }
    public static double jitVarDoubleArrayAndRelease(java.lang.invoke.VarHandle h, double[] a, int i, double v) { return (double) h.getAndBitwiseAndRelease(a, i, v); }
    public static double jitVarDoubleArrayXorAcquire(java.lang.invoke.VarHandle h, double[] a, int i, double v) { return (double) h.getAndBitwiseXorAcquire(a, i, v); }
    public static double jitVarDoubleArrayXorRelease(java.lang.invoke.VarHandle h, double[] a, int i, double v) { return (double) h.getAndBitwiseXorRelease(a, i, v); }
    public static byte jitVarByteGetOpaque(java.lang.invoke.VarHandle h, Hello r) { return (byte) h.getOpaque(r); }
    public static byte jitVarByteGetAcquire(java.lang.invoke.VarHandle h, Hello r) { return (byte) h.getAcquire(r); }
    public static byte jitVarByteGetVolatile(java.lang.invoke.VarHandle h, Hello r) { return (byte) h.getVolatile(r); }
    public static void jitVarByteSetOpaque(java.lang.invoke.VarHandle h, Hello r, byte v) { h.setOpaque(r, v); }
    public static void jitVarByteSetRelease(java.lang.invoke.VarHandle h, Hello r, byte v) { h.setRelease(r, v); }
    public static void jitVarByteSetVolatile(java.lang.invoke.VarHandle h, Hello r, byte v) { h.setVolatile(r, v); }
    public static boolean jitVarByteWeakPlain(java.lang.invoke.VarHandle h, Hello r, byte e, byte v) { return h.weakCompareAndSetPlain(r, e, v); }
    public static boolean jitVarByteWeakAcquire(java.lang.invoke.VarHandle h, Hello r, byte e, byte v) { return h.weakCompareAndSetAcquire(r, e, v); }
    public static boolean jitVarByteWeakRelease(java.lang.invoke.VarHandle h, Hello r, byte e, byte v) { return h.weakCompareAndSetRelease(r, e, v); }
    public static boolean jitVarByteWeakVolatile(java.lang.invoke.VarHandle h, Hello r, byte e, byte v) { return h.weakCompareAndSet(r, e, v); }
    public static byte jitVarByteExchangeAcquire(java.lang.invoke.VarHandle h, Hello r, byte e, byte v) { return (byte) h.compareAndExchangeAcquire(r, e, v); }
    public static byte jitVarByteExchangeRelease(java.lang.invoke.VarHandle h, Hello r, byte e, byte v) { return (byte) h.compareAndExchangeRelease(r, e, v); }
    public static byte jitVarByteSwapAcquire(java.lang.invoke.VarHandle h, Hello r, byte v) { return (byte) h.getAndSetAcquire(r, v); }
    public static byte jitVarByteSwapRelease(java.lang.invoke.VarHandle h, Hello r, byte v) { return (byte) h.getAndSetRelease(r, v); }
    public static byte jitVarByteAddAcquire(java.lang.invoke.VarHandle h, Hello r, byte v) { return (byte) h.getAndAddAcquire(r, v); }
    public static byte jitVarByteAddRelease(java.lang.invoke.VarHandle h, Hello r, byte v) { return (byte) h.getAndAddRelease(r, v); }
    public static byte jitVarByteOr(java.lang.invoke.VarHandle h, Hello r, byte v) { return (byte) h.getAndBitwiseOr(r, v); }
    public static byte jitVarByteOrAcquire(java.lang.invoke.VarHandle h, Hello r, byte v) { return (byte) h.getAndBitwiseOrAcquire(r, v); }
    public static byte jitVarByteOrRelease(java.lang.invoke.VarHandle h, Hello r, byte v) { return (byte) h.getAndBitwiseOrRelease(r, v); }
    public static byte jitVarByteAnd(java.lang.invoke.VarHandle h, Hello r, byte v) { return (byte) h.getAndBitwiseAnd(r, v); }
    public static byte jitVarByteAndAcquire(java.lang.invoke.VarHandle h, Hello r, byte v) { return (byte) h.getAndBitwiseAndAcquire(r, v); }
    public static byte jitVarByteAndRelease(java.lang.invoke.VarHandle h, Hello r, byte v) { return (byte) h.getAndBitwiseAndRelease(r, v); }
    public static byte jitVarByteXorAcquire(java.lang.invoke.VarHandle h, Hello r, byte v) { return (byte) h.getAndBitwiseXorAcquire(r, v); }
    public static byte jitVarByteXorRelease(java.lang.invoke.VarHandle h, Hello r, byte v) { return (byte) h.getAndBitwiseXorRelease(r, v); }
    public static byte jitVarStaticByteGetOpaque(java.lang.invoke.VarHandle h) { return (byte) h.getOpaque(); }
    public static byte jitVarStaticByteGetAcquire(java.lang.invoke.VarHandle h) { return (byte) h.getAcquire(); }
    public static byte jitVarStaticByteGetVolatile(java.lang.invoke.VarHandle h) { return (byte) h.getVolatile(); }
    public static void jitVarStaticByteSetOpaque(java.lang.invoke.VarHandle h, byte v) { h.setOpaque(v); }
    public static void jitVarStaticByteSetRelease(java.lang.invoke.VarHandle h, byte v) { h.setRelease(v); }
    public static void jitVarStaticByteSetVolatile(java.lang.invoke.VarHandle h, byte v) { h.setVolatile(v); }
    public static boolean jitVarStaticByteWeakPlain(java.lang.invoke.VarHandle h, byte e, byte v) { return h.weakCompareAndSetPlain(e, v); }
    public static boolean jitVarStaticByteWeakAcquire(java.lang.invoke.VarHandle h, byte e, byte v) { return h.weakCompareAndSetAcquire(e, v); }
    public static boolean jitVarStaticByteWeakRelease(java.lang.invoke.VarHandle h, byte e, byte v) { return h.weakCompareAndSetRelease(e, v); }
    public static boolean jitVarStaticByteWeakVolatile(java.lang.invoke.VarHandle h, byte e, byte v) { return h.weakCompareAndSet(e, v); }
    public static byte jitVarStaticByteExchangeAcquire(java.lang.invoke.VarHandle h, byte e, byte v) { return (byte) h.compareAndExchangeAcquire(e, v); }
    public static byte jitVarStaticByteExchangeRelease(java.lang.invoke.VarHandle h, byte e, byte v) { return (byte) h.compareAndExchangeRelease(e, v); }
    public static byte jitVarStaticByteSwapAcquire(java.lang.invoke.VarHandle h, byte v) { return (byte) h.getAndSetAcquire(v); }
    public static byte jitVarStaticByteSwapRelease(java.lang.invoke.VarHandle h, byte v) { return (byte) h.getAndSetRelease(v); }
    public static byte jitVarStaticByteAddAcquire(java.lang.invoke.VarHandle h, byte v) { return (byte) h.getAndAddAcquire(v); }
    public static byte jitVarStaticByteAddRelease(java.lang.invoke.VarHandle h, byte v) { return (byte) h.getAndAddRelease(v); }
    public static byte jitVarStaticByteOr(java.lang.invoke.VarHandle h, byte v) { return (byte) h.getAndBitwiseOr(v); }
    public static byte jitVarStaticByteOrAcquire(java.lang.invoke.VarHandle h, byte v) { return (byte) h.getAndBitwiseOrAcquire(v); }
    public static byte jitVarStaticByteOrRelease(java.lang.invoke.VarHandle h, byte v) { return (byte) h.getAndBitwiseOrRelease(v); }
    public static byte jitVarStaticByteAnd(java.lang.invoke.VarHandle h, byte v) { return (byte) h.getAndBitwiseAnd(v); }
    public static byte jitVarStaticByteAndAcquire(java.lang.invoke.VarHandle h, byte v) { return (byte) h.getAndBitwiseAndAcquire(v); }
    public static byte jitVarStaticByteAndRelease(java.lang.invoke.VarHandle h, byte v) { return (byte) h.getAndBitwiseAndRelease(v); }
    public static byte jitVarStaticByteXorAcquire(java.lang.invoke.VarHandle h, byte v) { return (byte) h.getAndBitwiseXorAcquire(v); }
    public static byte jitVarStaticByteXorRelease(java.lang.invoke.VarHandle h, byte v) { return (byte) h.getAndBitwiseXorRelease(v); }
    public static byte jitVarByteArrayGetOpaque(java.lang.invoke.VarHandle h, byte[] a, int i) { return (byte) h.getOpaque(a, i); }
    public static byte jitVarByteArrayGetAcquire(java.lang.invoke.VarHandle h, byte[] a, int i) { return (byte) h.getAcquire(a, i); }
    public static byte jitVarByteArrayGetVolatile(java.lang.invoke.VarHandle h, byte[] a, int i) { return (byte) h.getVolatile(a, i); }
    public static void jitVarByteArraySetOpaque(java.lang.invoke.VarHandle h, byte[] a, int i, byte v) { h.setOpaque(a, i, v); }
    public static void jitVarByteArraySetRelease(java.lang.invoke.VarHandle h, byte[] a, int i, byte v) { h.setRelease(a, i, v); }
    public static void jitVarByteArraySetVolatile(java.lang.invoke.VarHandle h, byte[] a, int i, byte v) { h.setVolatile(a, i, v); }
    public static boolean jitVarByteArrayWeakPlain(java.lang.invoke.VarHandle h, byte[] a, int i, byte e, byte v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarByteArrayWeakAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, byte e, byte v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarByteArrayWeakRelease(java.lang.invoke.VarHandle h, byte[] a, int i, byte e, byte v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarByteArrayWeakVolatile(java.lang.invoke.VarHandle h, byte[] a, int i, byte e, byte v) { return h.weakCompareAndSet(a, i, e, v); }
    public static byte jitVarByteArrayExchangeAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, byte e, byte v) { return (byte) h.compareAndExchangeAcquire(a, i, e, v); }
    public static byte jitVarByteArrayExchangeRelease(java.lang.invoke.VarHandle h, byte[] a, int i, byte e, byte v) { return (byte) h.compareAndExchangeRelease(a, i, e, v); }
    public static byte jitVarByteArraySwapAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, byte v) { return (byte) h.getAndSetAcquire(a, i, v); }
    public static byte jitVarByteArraySwapRelease(java.lang.invoke.VarHandle h, byte[] a, int i, byte v) { return (byte) h.getAndSetRelease(a, i, v); }
    public static byte jitVarByteArrayAddAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, byte v) { return (byte) h.getAndAddAcquire(a, i, v); }
    public static byte jitVarByteArrayAddRelease(java.lang.invoke.VarHandle h, byte[] a, int i, byte v) { return (byte) h.getAndAddRelease(a, i, v); }
    public static byte jitVarByteArrayOr(java.lang.invoke.VarHandle h, byte[] a, int i, byte v) { return (byte) h.getAndBitwiseOr(a, i, v); }
    public static byte jitVarByteArrayOrAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, byte v) { return (byte) h.getAndBitwiseOrAcquire(a, i, v); }
    public static byte jitVarByteArrayOrRelease(java.lang.invoke.VarHandle h, byte[] a, int i, byte v) { return (byte) h.getAndBitwiseOrRelease(a, i, v); }
    public static byte jitVarByteArrayAnd(java.lang.invoke.VarHandle h, byte[] a, int i, byte v) { return (byte) h.getAndBitwiseAnd(a, i, v); }
    public static byte jitVarByteArrayAndAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, byte v) { return (byte) h.getAndBitwiseAndAcquire(a, i, v); }
    public static byte jitVarByteArrayAndRelease(java.lang.invoke.VarHandle h, byte[] a, int i, byte v) { return (byte) h.getAndBitwiseAndRelease(a, i, v); }
    public static byte jitVarByteArrayXorAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, byte v) { return (byte) h.getAndBitwiseXorAcquire(a, i, v); }
    public static byte jitVarByteArrayXorRelease(java.lang.invoke.VarHandle h, byte[] a, int i, byte v) { return (byte) h.getAndBitwiseXorRelease(a, i, v); }
    public static boolean jitVarBooleanGetOpaque(java.lang.invoke.VarHandle h, Hello r) { return (boolean) h.getOpaque(r); }
    public static boolean jitVarBooleanGetAcquire(java.lang.invoke.VarHandle h, Hello r) { return (boolean) h.getAcquire(r); }
    public static boolean jitVarBooleanGetVolatile(java.lang.invoke.VarHandle h, Hello r) { return (boolean) h.getVolatile(r); }
    public static void jitVarBooleanSetOpaque(java.lang.invoke.VarHandle h, Hello r, boolean v) { h.setOpaque(r, v); }
    public static void jitVarBooleanSetRelease(java.lang.invoke.VarHandle h, Hello r, boolean v) { h.setRelease(r, v); }
    public static void jitVarBooleanSetVolatile(java.lang.invoke.VarHandle h, Hello r, boolean v) { h.setVolatile(r, v); }
    public static boolean jitVarBooleanWeakPlain(java.lang.invoke.VarHandle h, Hello r, boolean e, boolean v) { return h.weakCompareAndSetPlain(r, e, v); }
    public static boolean jitVarBooleanWeakAcquire(java.lang.invoke.VarHandle h, Hello r, boolean e, boolean v) { return h.weakCompareAndSetAcquire(r, e, v); }
    public static boolean jitVarBooleanWeakRelease(java.lang.invoke.VarHandle h, Hello r, boolean e, boolean v) { return h.weakCompareAndSetRelease(r, e, v); }
    public static boolean jitVarBooleanWeakVolatile(java.lang.invoke.VarHandle h, Hello r, boolean e, boolean v) { return h.weakCompareAndSet(r, e, v); }
    public static boolean jitVarBooleanExchangeAcquire(java.lang.invoke.VarHandle h, Hello r, boolean e, boolean v) { return (boolean) h.compareAndExchangeAcquire(r, e, v); }
    public static boolean jitVarBooleanExchangeRelease(java.lang.invoke.VarHandle h, Hello r, boolean e, boolean v) { return (boolean) h.compareAndExchangeRelease(r, e, v); }
    public static boolean jitVarBooleanSwapAcquire(java.lang.invoke.VarHandle h, Hello r, boolean v) { return (boolean) h.getAndSetAcquire(r, v); }
    public static boolean jitVarBooleanSwapRelease(java.lang.invoke.VarHandle h, Hello r, boolean v) { return (boolean) h.getAndSetRelease(r, v); }
    public static boolean jitVarBooleanAddAcquire(java.lang.invoke.VarHandle h, Hello r, boolean v) { return (boolean) h.getAndAddAcquire(r, v); }
    public static boolean jitVarBooleanAddRelease(java.lang.invoke.VarHandle h, Hello r, boolean v) { return (boolean) h.getAndAddRelease(r, v); }
    public static boolean jitVarBooleanOr(java.lang.invoke.VarHandle h, Hello r, boolean v) { return (boolean) h.getAndBitwiseOr(r, v); }
    public static boolean jitVarBooleanOrAcquire(java.lang.invoke.VarHandle h, Hello r, boolean v) { return (boolean) h.getAndBitwiseOrAcquire(r, v); }
    public static boolean jitVarBooleanOrRelease(java.lang.invoke.VarHandle h, Hello r, boolean v) { return (boolean) h.getAndBitwiseOrRelease(r, v); }
    public static boolean jitVarBooleanAnd(java.lang.invoke.VarHandle h, Hello r, boolean v) { return (boolean) h.getAndBitwiseAnd(r, v); }
    public static boolean jitVarBooleanAndAcquire(java.lang.invoke.VarHandle h, Hello r, boolean v) { return (boolean) h.getAndBitwiseAndAcquire(r, v); }
    public static boolean jitVarBooleanAndRelease(java.lang.invoke.VarHandle h, Hello r, boolean v) { return (boolean) h.getAndBitwiseAndRelease(r, v); }
    public static boolean jitVarBooleanXorAcquire(java.lang.invoke.VarHandle h, Hello r, boolean v) { return (boolean) h.getAndBitwiseXorAcquire(r, v); }
    public static boolean jitVarBooleanXorRelease(java.lang.invoke.VarHandle h, Hello r, boolean v) { return (boolean) h.getAndBitwiseXorRelease(r, v); }
    public static boolean jitVarStaticBooleanGetOpaque(java.lang.invoke.VarHandle h) { return (boolean) h.getOpaque(); }
    public static boolean jitVarStaticBooleanGetAcquire(java.lang.invoke.VarHandle h) { return (boolean) h.getAcquire(); }
    public static boolean jitVarStaticBooleanGetVolatile(java.lang.invoke.VarHandle h) { return (boolean) h.getVolatile(); }
    public static void jitVarStaticBooleanSetOpaque(java.lang.invoke.VarHandle h, boolean v) { h.setOpaque(v); }
    public static void jitVarStaticBooleanSetRelease(java.lang.invoke.VarHandle h, boolean v) { h.setRelease(v); }
    public static void jitVarStaticBooleanSetVolatile(java.lang.invoke.VarHandle h, boolean v) { h.setVolatile(v); }
    public static boolean jitVarStaticBooleanWeakPlain(java.lang.invoke.VarHandle h, boolean e, boolean v) { return h.weakCompareAndSetPlain(e, v); }
    public static boolean jitVarStaticBooleanWeakAcquire(java.lang.invoke.VarHandle h, boolean e, boolean v) { return h.weakCompareAndSetAcquire(e, v); }
    public static boolean jitVarStaticBooleanWeakRelease(java.lang.invoke.VarHandle h, boolean e, boolean v) { return h.weakCompareAndSetRelease(e, v); }
    public static boolean jitVarStaticBooleanWeakVolatile(java.lang.invoke.VarHandle h, boolean e, boolean v) { return h.weakCompareAndSet(e, v); }
    public static boolean jitVarStaticBooleanExchangeAcquire(java.lang.invoke.VarHandle h, boolean e, boolean v) { return (boolean) h.compareAndExchangeAcquire(e, v); }
    public static boolean jitVarStaticBooleanExchangeRelease(java.lang.invoke.VarHandle h, boolean e, boolean v) { return (boolean) h.compareAndExchangeRelease(e, v); }
    public static boolean jitVarStaticBooleanSwapAcquire(java.lang.invoke.VarHandle h, boolean v) { return (boolean) h.getAndSetAcquire(v); }
    public static boolean jitVarStaticBooleanSwapRelease(java.lang.invoke.VarHandle h, boolean v) { return (boolean) h.getAndSetRelease(v); }
    public static boolean jitVarStaticBooleanAddAcquire(java.lang.invoke.VarHandle h, boolean v) { return (boolean) h.getAndAddAcquire(v); }
    public static boolean jitVarStaticBooleanAddRelease(java.lang.invoke.VarHandle h, boolean v) { return (boolean) h.getAndAddRelease(v); }
    public static boolean jitVarStaticBooleanOr(java.lang.invoke.VarHandle h, boolean v) { return (boolean) h.getAndBitwiseOr(v); }
    public static boolean jitVarStaticBooleanOrAcquire(java.lang.invoke.VarHandle h, boolean v) { return (boolean) h.getAndBitwiseOrAcquire(v); }
    public static boolean jitVarStaticBooleanOrRelease(java.lang.invoke.VarHandle h, boolean v) { return (boolean) h.getAndBitwiseOrRelease(v); }
    public static boolean jitVarStaticBooleanAnd(java.lang.invoke.VarHandle h, boolean v) { return (boolean) h.getAndBitwiseAnd(v); }
    public static boolean jitVarStaticBooleanAndAcquire(java.lang.invoke.VarHandle h, boolean v) { return (boolean) h.getAndBitwiseAndAcquire(v); }
    public static boolean jitVarStaticBooleanAndRelease(java.lang.invoke.VarHandle h, boolean v) { return (boolean) h.getAndBitwiseAndRelease(v); }
    public static boolean jitVarStaticBooleanXorAcquire(java.lang.invoke.VarHandle h, boolean v) { return (boolean) h.getAndBitwiseXorAcquire(v); }
    public static boolean jitVarStaticBooleanXorRelease(java.lang.invoke.VarHandle h, boolean v) { return (boolean) h.getAndBitwiseXorRelease(v); }
    public static boolean jitVarBooleanArrayGetOpaque(java.lang.invoke.VarHandle h, boolean[] a, int i) { return (boolean) h.getOpaque(a, i); }
    public static boolean jitVarBooleanArrayGetAcquire(java.lang.invoke.VarHandle h, boolean[] a, int i) { return (boolean) h.getAcquire(a, i); }
    public static boolean jitVarBooleanArrayGetVolatile(java.lang.invoke.VarHandle h, boolean[] a, int i) { return (boolean) h.getVolatile(a, i); }
    public static void jitVarBooleanArraySetOpaque(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean v) { h.setOpaque(a, i, v); }
    public static void jitVarBooleanArraySetRelease(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean v) { h.setRelease(a, i, v); }
    public static void jitVarBooleanArraySetVolatile(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean v) { h.setVolatile(a, i, v); }
    public static boolean jitVarBooleanArrayWeakPlain(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean e, boolean v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarBooleanArrayWeakAcquire(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean e, boolean v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarBooleanArrayWeakRelease(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean e, boolean v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarBooleanArrayWeakVolatile(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean e, boolean v) { return h.weakCompareAndSet(a, i, e, v); }
    public static boolean jitVarBooleanArrayExchangeAcquire(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean e, boolean v) { return (boolean) h.compareAndExchangeAcquire(a, i, e, v); }
    public static boolean jitVarBooleanArrayExchangeRelease(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean e, boolean v) { return (boolean) h.compareAndExchangeRelease(a, i, e, v); }
    public static boolean jitVarBooleanArraySwapAcquire(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean v) { return (boolean) h.getAndSetAcquire(a, i, v); }
    public static boolean jitVarBooleanArraySwapRelease(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean v) { return (boolean) h.getAndSetRelease(a, i, v); }
    public static boolean jitVarBooleanArrayAddAcquire(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean v) { return (boolean) h.getAndAddAcquire(a, i, v); }
    public static boolean jitVarBooleanArrayAddRelease(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean v) { return (boolean) h.getAndAddRelease(a, i, v); }
    public static boolean jitVarBooleanArrayOr(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean v) { return (boolean) h.getAndBitwiseOr(a, i, v); }
    public static boolean jitVarBooleanArrayOrAcquire(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean v) { return (boolean) h.getAndBitwiseOrAcquire(a, i, v); }
    public static boolean jitVarBooleanArrayOrRelease(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean v) { return (boolean) h.getAndBitwiseOrRelease(a, i, v); }
    public static boolean jitVarBooleanArrayAnd(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean v) { return (boolean) h.getAndBitwiseAnd(a, i, v); }
    public static boolean jitVarBooleanArrayAndAcquire(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean v) { return (boolean) h.getAndBitwiseAndAcquire(a, i, v); }
    public static boolean jitVarBooleanArrayAndRelease(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean v) { return (boolean) h.getAndBitwiseAndRelease(a, i, v); }
    public static boolean jitVarBooleanArrayXorAcquire(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean v) { return (boolean) h.getAndBitwiseXorAcquire(a, i, v); }
    public static boolean jitVarBooleanArrayXorRelease(java.lang.invoke.VarHandle h, boolean[] a, int i, boolean v) { return (boolean) h.getAndBitwiseXorRelease(a, i, v); }
    public static short jitVarShortGetOpaque(java.lang.invoke.VarHandle h, Hello r) { return (short) h.getOpaque(r); }
    public static short jitVarShortGetAcquire(java.lang.invoke.VarHandle h, Hello r) { return (short) h.getAcquire(r); }
    public static short jitVarShortGetVolatile(java.lang.invoke.VarHandle h, Hello r) { return (short) h.getVolatile(r); }
    public static void jitVarShortSetOpaque(java.lang.invoke.VarHandle h, Hello r, short v) { h.setOpaque(r, v); }
    public static void jitVarShortSetRelease(java.lang.invoke.VarHandle h, Hello r, short v) { h.setRelease(r, v); }
    public static void jitVarShortSetVolatile(java.lang.invoke.VarHandle h, Hello r, short v) { h.setVolatile(r, v); }
    public static boolean jitVarShortWeakPlain(java.lang.invoke.VarHandle h, Hello r, short e, short v) { return h.weakCompareAndSetPlain(r, e, v); }
    public static boolean jitVarShortWeakAcquire(java.lang.invoke.VarHandle h, Hello r, short e, short v) { return h.weakCompareAndSetAcquire(r, e, v); }
    public static boolean jitVarShortWeakRelease(java.lang.invoke.VarHandle h, Hello r, short e, short v) { return h.weakCompareAndSetRelease(r, e, v); }
    public static boolean jitVarShortWeakVolatile(java.lang.invoke.VarHandle h, Hello r, short e, short v) { return h.weakCompareAndSet(r, e, v); }
    public static short jitVarShortExchangeAcquire(java.lang.invoke.VarHandle h, Hello r, short e, short v) { return (short) h.compareAndExchangeAcquire(r, e, v); }
    public static short jitVarShortExchangeRelease(java.lang.invoke.VarHandle h, Hello r, short e, short v) { return (short) h.compareAndExchangeRelease(r, e, v); }
    public static short jitVarShortSwapAcquire(java.lang.invoke.VarHandle h, Hello r, short v) { return (short) h.getAndSetAcquire(r, v); }
    public static short jitVarShortSwapRelease(java.lang.invoke.VarHandle h, Hello r, short v) { return (short) h.getAndSetRelease(r, v); }
    public static short jitVarShortAddAcquire(java.lang.invoke.VarHandle h, Hello r, short v) { return (short) h.getAndAddAcquire(r, v); }
    public static short jitVarShortAddRelease(java.lang.invoke.VarHandle h, Hello r, short v) { return (short) h.getAndAddRelease(r, v); }
    public static short jitVarShortOr(java.lang.invoke.VarHandle h, Hello r, short v) { return (short) h.getAndBitwiseOr(r, v); }
    public static short jitVarShortOrAcquire(java.lang.invoke.VarHandle h, Hello r, short v) { return (short) h.getAndBitwiseOrAcquire(r, v); }
    public static short jitVarShortOrRelease(java.lang.invoke.VarHandle h, Hello r, short v) { return (short) h.getAndBitwiseOrRelease(r, v); }
    public static short jitVarShortAnd(java.lang.invoke.VarHandle h, Hello r, short v) { return (short) h.getAndBitwiseAnd(r, v); }
    public static short jitVarShortAndAcquire(java.lang.invoke.VarHandle h, Hello r, short v) { return (short) h.getAndBitwiseAndAcquire(r, v); }
    public static short jitVarShortAndRelease(java.lang.invoke.VarHandle h, Hello r, short v) { return (short) h.getAndBitwiseAndRelease(r, v); }
    public static short jitVarShortXorAcquire(java.lang.invoke.VarHandle h, Hello r, short v) { return (short) h.getAndBitwiseXorAcquire(r, v); }
    public static short jitVarShortXorRelease(java.lang.invoke.VarHandle h, Hello r, short v) { return (short) h.getAndBitwiseXorRelease(r, v); }
    public static short jitVarStaticShortGetOpaque(java.lang.invoke.VarHandle h) { return (short) h.getOpaque(); }
    public static short jitVarStaticShortGetAcquire(java.lang.invoke.VarHandle h) { return (short) h.getAcquire(); }
    public static short jitVarStaticShortGetVolatile(java.lang.invoke.VarHandle h) { return (short) h.getVolatile(); }
    public static void jitVarStaticShortSetOpaque(java.lang.invoke.VarHandle h, short v) { h.setOpaque(v); }
    public static void jitVarStaticShortSetRelease(java.lang.invoke.VarHandle h, short v) { h.setRelease(v); }
    public static void jitVarStaticShortSetVolatile(java.lang.invoke.VarHandle h, short v) { h.setVolatile(v); }
    public static boolean jitVarStaticShortWeakPlain(java.lang.invoke.VarHandle h, short e, short v) { return h.weakCompareAndSetPlain(e, v); }
    public static boolean jitVarStaticShortWeakAcquire(java.lang.invoke.VarHandle h, short e, short v) { return h.weakCompareAndSetAcquire(e, v); }
    public static boolean jitVarStaticShortWeakRelease(java.lang.invoke.VarHandle h, short e, short v) { return h.weakCompareAndSetRelease(e, v); }
    public static boolean jitVarStaticShortWeakVolatile(java.lang.invoke.VarHandle h, short e, short v) { return h.weakCompareAndSet(e, v); }
    public static short jitVarStaticShortExchangeAcquire(java.lang.invoke.VarHandle h, short e, short v) { return (short) h.compareAndExchangeAcquire(e, v); }
    public static short jitVarStaticShortExchangeRelease(java.lang.invoke.VarHandle h, short e, short v) { return (short) h.compareAndExchangeRelease(e, v); }
    public static short jitVarStaticShortSwapAcquire(java.lang.invoke.VarHandle h, short v) { return (short) h.getAndSetAcquire(v); }
    public static short jitVarStaticShortSwapRelease(java.lang.invoke.VarHandle h, short v) { return (short) h.getAndSetRelease(v); }
    public static short jitVarStaticShortAddAcquire(java.lang.invoke.VarHandle h, short v) { return (short) h.getAndAddAcquire(v); }
    public static short jitVarStaticShortAddRelease(java.lang.invoke.VarHandle h, short v) { return (short) h.getAndAddRelease(v); }
    public static short jitVarStaticShortOr(java.lang.invoke.VarHandle h, short v) { return (short) h.getAndBitwiseOr(v); }
    public static short jitVarStaticShortOrAcquire(java.lang.invoke.VarHandle h, short v) { return (short) h.getAndBitwiseOrAcquire(v); }
    public static short jitVarStaticShortOrRelease(java.lang.invoke.VarHandle h, short v) { return (short) h.getAndBitwiseOrRelease(v); }
    public static short jitVarStaticShortAnd(java.lang.invoke.VarHandle h, short v) { return (short) h.getAndBitwiseAnd(v); }
    public static short jitVarStaticShortAndAcquire(java.lang.invoke.VarHandle h, short v) { return (short) h.getAndBitwiseAndAcquire(v); }
    public static short jitVarStaticShortAndRelease(java.lang.invoke.VarHandle h, short v) { return (short) h.getAndBitwiseAndRelease(v); }
    public static short jitVarStaticShortXorAcquire(java.lang.invoke.VarHandle h, short v) { return (short) h.getAndBitwiseXorAcquire(v); }
    public static short jitVarStaticShortXorRelease(java.lang.invoke.VarHandle h, short v) { return (short) h.getAndBitwiseXorRelease(v); }
    public static short jitVarShortArrayGetOpaque(java.lang.invoke.VarHandle h, short[] a, int i) { return (short) h.getOpaque(a, i); }
    public static short jitVarShortArrayGetAcquire(java.lang.invoke.VarHandle h, short[] a, int i) { return (short) h.getAcquire(a, i); }
    public static short jitVarShortArrayGetVolatile(java.lang.invoke.VarHandle h, short[] a, int i) { return (short) h.getVolatile(a, i); }
    public static void jitVarShortArraySetOpaque(java.lang.invoke.VarHandle h, short[] a, int i, short v) { h.setOpaque(a, i, v); }
    public static void jitVarShortArraySetRelease(java.lang.invoke.VarHandle h, short[] a, int i, short v) { h.setRelease(a, i, v); }
    public static void jitVarShortArraySetVolatile(java.lang.invoke.VarHandle h, short[] a, int i, short v) { h.setVolatile(a, i, v); }
    public static boolean jitVarShortArrayWeakPlain(java.lang.invoke.VarHandle h, short[] a, int i, short e, short v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarShortArrayWeakAcquire(java.lang.invoke.VarHandle h, short[] a, int i, short e, short v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarShortArrayWeakRelease(java.lang.invoke.VarHandle h, short[] a, int i, short e, short v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarShortArrayWeakVolatile(java.lang.invoke.VarHandle h, short[] a, int i, short e, short v) { return h.weakCompareAndSet(a, i, e, v); }
    public static short jitVarShortArrayExchangeAcquire(java.lang.invoke.VarHandle h, short[] a, int i, short e, short v) { return (short) h.compareAndExchangeAcquire(a, i, e, v); }
    public static short jitVarShortArrayExchangeRelease(java.lang.invoke.VarHandle h, short[] a, int i, short e, short v) { return (short) h.compareAndExchangeRelease(a, i, e, v); }
    public static short jitVarShortArraySwapAcquire(java.lang.invoke.VarHandle h, short[] a, int i, short v) { return (short) h.getAndSetAcquire(a, i, v); }
    public static short jitVarShortArraySwapRelease(java.lang.invoke.VarHandle h, short[] a, int i, short v) { return (short) h.getAndSetRelease(a, i, v); }
    public static short jitVarShortArrayAddAcquire(java.lang.invoke.VarHandle h, short[] a, int i, short v) { return (short) h.getAndAddAcquire(a, i, v); }
    public static short jitVarShortArrayAddRelease(java.lang.invoke.VarHandle h, short[] a, int i, short v) { return (short) h.getAndAddRelease(a, i, v); }
    public static short jitVarShortArrayOr(java.lang.invoke.VarHandle h, short[] a, int i, short v) { return (short) h.getAndBitwiseOr(a, i, v); }
    public static short jitVarShortArrayOrAcquire(java.lang.invoke.VarHandle h, short[] a, int i, short v) { return (short) h.getAndBitwiseOrAcquire(a, i, v); }
    public static short jitVarShortArrayOrRelease(java.lang.invoke.VarHandle h, short[] a, int i, short v) { return (short) h.getAndBitwiseOrRelease(a, i, v); }
    public static short jitVarShortArrayAnd(java.lang.invoke.VarHandle h, short[] a, int i, short v) { return (short) h.getAndBitwiseAnd(a, i, v); }
    public static short jitVarShortArrayAndAcquire(java.lang.invoke.VarHandle h, short[] a, int i, short v) { return (short) h.getAndBitwiseAndAcquire(a, i, v); }
    public static short jitVarShortArrayAndRelease(java.lang.invoke.VarHandle h, short[] a, int i, short v) { return (short) h.getAndBitwiseAndRelease(a, i, v); }
    public static short jitVarShortArrayXorAcquire(java.lang.invoke.VarHandle h, short[] a, int i, short v) { return (short) h.getAndBitwiseXorAcquire(a, i, v); }
    public static short jitVarShortArrayXorRelease(java.lang.invoke.VarHandle h, short[] a, int i, short v) { return (short) h.getAndBitwiseXorRelease(a, i, v); }
    public static char jitVarCharGetOpaque(java.lang.invoke.VarHandle h, Hello r) { return (char) h.getOpaque(r); }
    public static char jitVarCharGetAcquire(java.lang.invoke.VarHandle h, Hello r) { return (char) h.getAcquire(r); }
    public static char jitVarCharGetVolatile(java.lang.invoke.VarHandle h, Hello r) { return (char) h.getVolatile(r); }
    public static void jitVarCharSetOpaque(java.lang.invoke.VarHandle h, Hello r, char v) { h.setOpaque(r, v); }
    public static void jitVarCharSetRelease(java.lang.invoke.VarHandle h, Hello r, char v) { h.setRelease(r, v); }
    public static void jitVarCharSetVolatile(java.lang.invoke.VarHandle h, Hello r, char v) { h.setVolatile(r, v); }
    public static boolean jitVarCharWeakPlain(java.lang.invoke.VarHandle h, Hello r, char e, char v) { return h.weakCompareAndSetPlain(r, e, v); }
    public static boolean jitVarCharWeakAcquire(java.lang.invoke.VarHandle h, Hello r, char e, char v) { return h.weakCompareAndSetAcquire(r, e, v); }
    public static boolean jitVarCharWeakRelease(java.lang.invoke.VarHandle h, Hello r, char e, char v) { return h.weakCompareAndSetRelease(r, e, v); }
    public static boolean jitVarCharWeakVolatile(java.lang.invoke.VarHandle h, Hello r, char e, char v) { return h.weakCompareAndSet(r, e, v); }
    public static char jitVarCharExchangeAcquire(java.lang.invoke.VarHandle h, Hello r, char e, char v) { return (char) h.compareAndExchangeAcquire(r, e, v); }
    public static char jitVarCharExchangeRelease(java.lang.invoke.VarHandle h, Hello r, char e, char v) { return (char) h.compareAndExchangeRelease(r, e, v); }
    public static char jitVarCharSwapAcquire(java.lang.invoke.VarHandle h, Hello r, char v) { return (char) h.getAndSetAcquire(r, v); }
    public static char jitVarCharSwapRelease(java.lang.invoke.VarHandle h, Hello r, char v) { return (char) h.getAndSetRelease(r, v); }
    public static char jitVarCharAddAcquire(java.lang.invoke.VarHandle h, Hello r, char v) { return (char) h.getAndAddAcquire(r, v); }
    public static char jitVarCharAddRelease(java.lang.invoke.VarHandle h, Hello r, char v) { return (char) h.getAndAddRelease(r, v); }
    public static char jitVarCharOr(java.lang.invoke.VarHandle h, Hello r, char v) { return (char) h.getAndBitwiseOr(r, v); }
    public static char jitVarCharOrAcquire(java.lang.invoke.VarHandle h, Hello r, char v) { return (char) h.getAndBitwiseOrAcquire(r, v); }
    public static char jitVarCharOrRelease(java.lang.invoke.VarHandle h, Hello r, char v) { return (char) h.getAndBitwiseOrRelease(r, v); }
    public static char jitVarCharAnd(java.lang.invoke.VarHandle h, Hello r, char v) { return (char) h.getAndBitwiseAnd(r, v); }
    public static char jitVarCharAndAcquire(java.lang.invoke.VarHandle h, Hello r, char v) { return (char) h.getAndBitwiseAndAcquire(r, v); }
    public static char jitVarCharAndRelease(java.lang.invoke.VarHandle h, Hello r, char v) { return (char) h.getAndBitwiseAndRelease(r, v); }
    public static char jitVarCharXorAcquire(java.lang.invoke.VarHandle h, Hello r, char v) { return (char) h.getAndBitwiseXorAcquire(r, v); }
    public static char jitVarCharXorRelease(java.lang.invoke.VarHandle h, Hello r, char v) { return (char) h.getAndBitwiseXorRelease(r, v); }
    public static char jitVarStaticCharGetOpaque(java.lang.invoke.VarHandle h) { return (char) h.getOpaque(); }
    public static char jitVarStaticCharGetAcquire(java.lang.invoke.VarHandle h) { return (char) h.getAcquire(); }
    public static char jitVarStaticCharGetVolatile(java.lang.invoke.VarHandle h) { return (char) h.getVolatile(); }
    public static void jitVarStaticCharSetOpaque(java.lang.invoke.VarHandle h, char v) { h.setOpaque(v); }
    public static void jitVarStaticCharSetRelease(java.lang.invoke.VarHandle h, char v) { h.setRelease(v); }
    public static void jitVarStaticCharSetVolatile(java.lang.invoke.VarHandle h, char v) { h.setVolatile(v); }
    public static boolean jitVarStaticCharWeakPlain(java.lang.invoke.VarHandle h, char e, char v) { return h.weakCompareAndSetPlain(e, v); }
    public static boolean jitVarStaticCharWeakAcquire(java.lang.invoke.VarHandle h, char e, char v) { return h.weakCompareAndSetAcquire(e, v); }
    public static boolean jitVarStaticCharWeakRelease(java.lang.invoke.VarHandle h, char e, char v) { return h.weakCompareAndSetRelease(e, v); }
    public static boolean jitVarStaticCharWeakVolatile(java.lang.invoke.VarHandle h, char e, char v) { return h.weakCompareAndSet(e, v); }
    public static char jitVarStaticCharExchangeAcquire(java.lang.invoke.VarHandle h, char e, char v) { return (char) h.compareAndExchangeAcquire(e, v); }
    public static char jitVarStaticCharExchangeRelease(java.lang.invoke.VarHandle h, char e, char v) { return (char) h.compareAndExchangeRelease(e, v); }
    public static char jitVarStaticCharSwapAcquire(java.lang.invoke.VarHandle h, char v) { return (char) h.getAndSetAcquire(v); }
    public static char jitVarStaticCharSwapRelease(java.lang.invoke.VarHandle h, char v) { return (char) h.getAndSetRelease(v); }
    public static char jitVarStaticCharAddAcquire(java.lang.invoke.VarHandle h, char v) { return (char) h.getAndAddAcquire(v); }
    public static char jitVarStaticCharAddRelease(java.lang.invoke.VarHandle h, char v) { return (char) h.getAndAddRelease(v); }
    public static char jitVarStaticCharOr(java.lang.invoke.VarHandle h, char v) { return (char) h.getAndBitwiseOr(v); }
    public static char jitVarStaticCharOrAcquire(java.lang.invoke.VarHandle h, char v) { return (char) h.getAndBitwiseOrAcquire(v); }
    public static char jitVarStaticCharOrRelease(java.lang.invoke.VarHandle h, char v) { return (char) h.getAndBitwiseOrRelease(v); }
    public static char jitVarStaticCharAnd(java.lang.invoke.VarHandle h, char v) { return (char) h.getAndBitwiseAnd(v); }
    public static char jitVarStaticCharAndAcquire(java.lang.invoke.VarHandle h, char v) { return (char) h.getAndBitwiseAndAcquire(v); }
    public static char jitVarStaticCharAndRelease(java.lang.invoke.VarHandle h, char v) { return (char) h.getAndBitwiseAndRelease(v); }
    public static char jitVarStaticCharXorAcquire(java.lang.invoke.VarHandle h, char v) { return (char) h.getAndBitwiseXorAcquire(v); }
    public static char jitVarStaticCharXorRelease(java.lang.invoke.VarHandle h, char v) { return (char) h.getAndBitwiseXorRelease(v); }
    public static char jitVarCharArrayGetOpaque(java.lang.invoke.VarHandle h, char[] a, int i) { return (char) h.getOpaque(a, i); }
    public static char jitVarCharArrayGetAcquire(java.lang.invoke.VarHandle h, char[] a, int i) { return (char) h.getAcquire(a, i); }
    public static char jitVarCharArrayGetVolatile(java.lang.invoke.VarHandle h, char[] a, int i) { return (char) h.getVolatile(a, i); }
    public static void jitVarCharArraySetOpaque(java.lang.invoke.VarHandle h, char[] a, int i, char v) { h.setOpaque(a, i, v); }
    public static void jitVarCharArraySetRelease(java.lang.invoke.VarHandle h, char[] a, int i, char v) { h.setRelease(a, i, v); }
    public static void jitVarCharArraySetVolatile(java.lang.invoke.VarHandle h, char[] a, int i, char v) { h.setVolatile(a, i, v); }
    public static boolean jitVarCharArrayWeakPlain(java.lang.invoke.VarHandle h, char[] a, int i, char e, char v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarCharArrayWeakAcquire(java.lang.invoke.VarHandle h, char[] a, int i, char e, char v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarCharArrayWeakRelease(java.lang.invoke.VarHandle h, char[] a, int i, char e, char v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarCharArrayWeakVolatile(java.lang.invoke.VarHandle h, char[] a, int i, char e, char v) { return h.weakCompareAndSet(a, i, e, v); }
    public static char jitVarCharArrayExchangeAcquire(java.lang.invoke.VarHandle h, char[] a, int i, char e, char v) { return (char) h.compareAndExchangeAcquire(a, i, e, v); }
    public static char jitVarCharArrayExchangeRelease(java.lang.invoke.VarHandle h, char[] a, int i, char e, char v) { return (char) h.compareAndExchangeRelease(a, i, e, v); }
    public static char jitVarCharArraySwapAcquire(java.lang.invoke.VarHandle h, char[] a, int i, char v) { return (char) h.getAndSetAcquire(a, i, v); }
    public static char jitVarCharArraySwapRelease(java.lang.invoke.VarHandle h, char[] a, int i, char v) { return (char) h.getAndSetRelease(a, i, v); }
    public static char jitVarCharArrayAddAcquire(java.lang.invoke.VarHandle h, char[] a, int i, char v) { return (char) h.getAndAddAcquire(a, i, v); }
    public static char jitVarCharArrayAddRelease(java.lang.invoke.VarHandle h, char[] a, int i, char v) { return (char) h.getAndAddRelease(a, i, v); }
    public static char jitVarCharArrayOr(java.lang.invoke.VarHandle h, char[] a, int i, char v) { return (char) h.getAndBitwiseOr(a, i, v); }
    public static char jitVarCharArrayOrAcquire(java.lang.invoke.VarHandle h, char[] a, int i, char v) { return (char) h.getAndBitwiseOrAcquire(a, i, v); }
    public static char jitVarCharArrayOrRelease(java.lang.invoke.VarHandle h, char[] a, int i, char v) { return (char) h.getAndBitwiseOrRelease(a, i, v); }
    public static char jitVarCharArrayAnd(java.lang.invoke.VarHandle h, char[] a, int i, char v) { return (char) h.getAndBitwiseAnd(a, i, v); }
    public static char jitVarCharArrayAndAcquire(java.lang.invoke.VarHandle h, char[] a, int i, char v) { return (char) h.getAndBitwiseAndAcquire(a, i, v); }
    public static char jitVarCharArrayAndRelease(java.lang.invoke.VarHandle h, char[] a, int i, char v) { return (char) h.getAndBitwiseAndRelease(a, i, v); }
    public static char jitVarCharArrayXorAcquire(java.lang.invoke.VarHandle h, char[] a, int i, char v) { return (char) h.getAndBitwiseXorAcquire(a, i, v); }
    public static char jitVarCharArrayXorRelease(java.lang.invoke.VarHandle h, char[] a, int i, char v) { return (char) h.getAndBitwiseXorRelease(a, i, v); }
    public static java.lang.invoke.VarHandle jitVarDoubleHandle(boolean stat) throws Exception {
        String method = stat ? "findStaticVarHandle" : "findVarHandle";
        return (java.lang.invoke.VarHandle) java.lang.invoke.MethodHandles.Lookup.class
                .getMethod(method, Class.class, String.class, Class.class)
                .invoke(java.lang.invoke.MethodHandles.lookup(), Hello.class,
                        stat ? "jitVarStaticDouble" : "jitVarDouble", double.class);
    }
    public static double jitVarDoubleGet(java.lang.invoke.VarHandle handle, Hello receiver) {
        return (double) handle.get(receiver);
    }
    public static void jitVarDoubleSet(java.lang.invoke.VarHandle handle, Hello receiver, double value) {
        handle.set(receiver, value);
    }
    public static boolean jitVarDoubleCas(java.lang.invoke.VarHandle handle, Hello receiver,
            double expected, double value) {
        return handle.compareAndSet(receiver, expected, value);
    }
    public static double jitVarDoubleAdd(java.lang.invoke.VarHandle handle, Hello receiver, double value) {
        return (double) handle.getAndAdd(receiver, value);
    }
    public static double jitVarStaticDoubleGet(java.lang.invoke.VarHandle handle) {
        return (double) handle.get();
    }
    public static void jitVarStaticDoubleSet(java.lang.invoke.VarHandle handle, double value) {
        handle.set(value);
    }
    public static boolean jitVarStaticDoubleCas(java.lang.invoke.VarHandle handle,
            double expected, double value) {
        return handle.compareAndSet(expected, value);
    }
    public static double jitVarStaticDoubleAdd(java.lang.invoke.VarHandle handle, double value) {
        return (double) handle.getAndAdd(value);
    }
    public static java.lang.invoke.VarHandle jitVarFloatHandle(boolean stat) throws Exception {
        String method = stat ? "findStaticVarHandle" : "findVarHandle";
        return (java.lang.invoke.VarHandle) java.lang.invoke.MethodHandles.Lookup.class
                .getMethod(method, Class.class, String.class, Class.class)
                .invoke(java.lang.invoke.MethodHandles.lookup(), Hello.class,
                        stat ? "jitVarStaticFloat" : "jitVarFloat", float.class);
    }
    public static float jitVarFloatGet(java.lang.invoke.VarHandle handle, Hello receiver) {
        return (float) handle.get(receiver);
    }
    public static void jitVarFloatSet(java.lang.invoke.VarHandle handle, Hello receiver, float value) {
        handle.set(receiver, value);
    }
    public static boolean jitVarFloatCas(java.lang.invoke.VarHandle handle, Hello receiver,
            float expected, float value) {
        return handle.compareAndSet(receiver, expected, value);
    }
    public static float jitVarFloatAdd(java.lang.invoke.VarHandle handle, Hello receiver, float value) {
        return (float) handle.getAndAdd(receiver, value);
    }
    public static float jitVarStaticFloatGet(java.lang.invoke.VarHandle handle) {
        return (float) handle.get();
    }
    public static void jitVarStaticFloatSet(java.lang.invoke.VarHandle handle, float value) {
        handle.set(value);
    }
    public static boolean jitVarStaticFloatCas(java.lang.invoke.VarHandle handle,
            float expected, float value) {
        return handle.compareAndSet(expected, value);
    }
    public static float jitVarStaticFloatAdd(java.lang.invoke.VarHandle handle, float value) {
        return (float) handle.getAndAdd(value);
    }
    public static java.lang.invoke.VarHandle jitVarByteHandle(boolean stat) throws Exception {
        String method = stat ? "findStaticVarHandle" : "findVarHandle";
        return (java.lang.invoke.VarHandle) java.lang.invoke.MethodHandles.Lookup.class
                .getMethod(method, Class.class, String.class, Class.class)
                .invoke(java.lang.invoke.MethodHandles.lookup(), Hello.class,
                        stat ? "jitVarStaticByte" : "jitVarByte", byte.class);
    }
    public static byte jitVarByteGet(java.lang.invoke.VarHandle handle, Hello receiver) {
        return (byte) handle.get(receiver);
    }
    public static void jitVarByteSet(java.lang.invoke.VarHandle handle, Hello receiver, byte value) {
        handle.set(receiver, value);
    }
    public static boolean jitVarByteCas(java.lang.invoke.VarHandle handle, Hello receiver,
            byte expected, byte value) {
        return handle.compareAndSet(receiver, expected, value);
    }
    public static byte jitVarByteAdd(java.lang.invoke.VarHandle handle, Hello receiver, byte value) {
        return (byte) handle.getAndAdd(receiver, value);
    }
    public static byte jitVarStaticByteGet(java.lang.invoke.VarHandle handle) {
        return (byte) handle.get();
    }
    public static void jitVarStaticByteSet(java.lang.invoke.VarHandle handle, byte value) {
        handle.set(value);
    }
    public static boolean jitVarStaticByteCas(java.lang.invoke.VarHandle handle, byte expected, byte value) {
        return handle.compareAndSet(expected, value);
    }
    public static byte jitVarStaticByteAdd(java.lang.invoke.VarHandle handle, byte value) {
        return (byte) handle.getAndAdd(value);
    }
    public static java.lang.invoke.VarHandle jitVarBooleanHandle(boolean stat) throws Exception {
        String method = stat ? "findStaticVarHandle" : "findVarHandle";
        return (java.lang.invoke.VarHandle) java.lang.invoke.MethodHandles.Lookup.class
                .getMethod(method, Class.class, String.class, Class.class)
                .invoke(java.lang.invoke.MethodHandles.lookup(), Hello.class,
                        stat ? "jitVarStaticBoolean" : "jitVarBoolean", boolean.class);
    }
    public static boolean jitVarBooleanGet(java.lang.invoke.VarHandle handle, Hello receiver) {
        return (boolean) handle.get(receiver);
    }
    public static void jitVarBooleanSet(java.lang.invoke.VarHandle handle, Hello receiver, boolean value) {
        handle.set(receiver, value);
    }
    public static boolean jitVarBooleanCas(java.lang.invoke.VarHandle handle, Hello receiver,
            boolean expected, boolean value) {
        return handle.compareAndSet(receiver, expected, value);
    }
    public static boolean jitVarBooleanXor(java.lang.invoke.VarHandle handle, Hello receiver, boolean value) {
        return (boolean) handle.getAndBitwiseXor(receiver, value);
    }
    public static boolean jitVarStaticBooleanGet(java.lang.invoke.VarHandle handle) {
        return (boolean) handle.get();
    }
    public static void jitVarStaticBooleanSet(java.lang.invoke.VarHandle handle, boolean value) {
        handle.set(value);
    }
    public static boolean jitVarStaticBooleanCas(java.lang.invoke.VarHandle handle,
            boolean expected, boolean value) {
        return handle.compareAndSet(expected, value);
    }
    public static boolean jitVarStaticBooleanXor(java.lang.invoke.VarHandle handle, boolean value) {
        return (boolean) handle.getAndBitwiseXor(value);
    }
    public static java.lang.invoke.VarHandle jitVarShortHandle(boolean stat) throws Exception {
        String method = stat ? "findStaticVarHandle" : "findVarHandle";
        return (java.lang.invoke.VarHandle) java.lang.invoke.MethodHandles.Lookup.class
                .getMethod(method, Class.class, String.class, Class.class)
                .invoke(java.lang.invoke.MethodHandles.lookup(), Hello.class,
                        stat ? "jitVarStaticShort" : "jitVarShort", short.class);
    }
    public static short jitVarShortGet(java.lang.invoke.VarHandle handle, Hello receiver) {
        return (short) handle.get(receiver);
    }
    public static void jitVarShortSet(java.lang.invoke.VarHandle handle, Hello receiver, short value) {
        handle.set(receiver, value);
    }
    public static boolean jitVarShortCas(java.lang.invoke.VarHandle handle, Hello receiver,
            short expected, short value) {
        return handle.compareAndSet(receiver, expected, value);
    }
    public static short jitVarShortAdd(java.lang.invoke.VarHandle handle, Hello receiver, short value) {
        return (short) handle.getAndAdd(receiver, value);
    }
    public static short jitVarStaticShortGet(java.lang.invoke.VarHandle handle) {
        return (short) handle.get();
    }
    public static void jitVarStaticShortSet(java.lang.invoke.VarHandle handle, short value) {
        handle.set(value);
    }
    public static boolean jitVarStaticShortCas(java.lang.invoke.VarHandle handle, short expected, short value) {
        return handle.compareAndSet(expected, value);
    }
    public static short jitVarStaticShortAdd(java.lang.invoke.VarHandle handle, short value) {
        return (short) handle.getAndAdd(value);
    }
    public static java.lang.invoke.VarHandle jitVarCharHandle(boolean stat) throws Exception {
        String method = stat ? "findStaticVarHandle" : "findVarHandle";
        return (java.lang.invoke.VarHandle) java.lang.invoke.MethodHandles.Lookup.class
                .getMethod(method, Class.class, String.class, Class.class)
                .invoke(java.lang.invoke.MethodHandles.lookup(), Hello.class,
                        stat ? "jitVarStaticChar" : "jitVarChar", char.class);
    }
    public static char jitVarCharGet(java.lang.invoke.VarHandle handle, Hello receiver) {
        return (char) handle.get(receiver);
    }
    public static void jitVarCharSet(java.lang.invoke.VarHandle handle, Hello receiver, char value) {
        handle.set(receiver, value);
    }
    public static boolean jitVarCharCas(java.lang.invoke.VarHandle handle, Hello receiver,
            char expected, char value) {
        return handle.compareAndSet(receiver, expected, value);
    }
    public static char jitVarCharAdd(java.lang.invoke.VarHandle handle, Hello receiver, char value) {
        return (char) handle.getAndAdd(receiver, value);
    }
    public static char jitVarStaticCharGet(java.lang.invoke.VarHandle handle) {
        return (char) handle.get();
    }
    public static void jitVarStaticCharSet(java.lang.invoke.VarHandle handle, char value) {
        handle.set(value);
    }
    public static boolean jitVarStaticCharCas(java.lang.invoke.VarHandle handle, char expected, char value) {
        return handle.compareAndSet(expected, value);
    }
    public static char jitVarStaticCharAdd(java.lang.invoke.VarHandle handle, char value) {
        return (char) handle.getAndAdd(value);
    }
    public static java.lang.invoke.VarHandle jitVarArrayHandle(Class<?> arrayClass) throws Exception {
        return (java.lang.invoke.VarHandle) java.lang.invoke.MethodHandles.class
                .getMethod("arrayElementVarHandle", Class.class)
                .invoke(null, arrayClass);
    }
    public static java.lang.invoke.VarHandle jitVarByteArrayViewHandle(
            Class<?> viewArrayClass, boolean littleEndian) throws Exception {
        return (java.lang.invoke.VarHandle) java.lang.invoke.MethodHandles.class
                .getMethod("byteArrayViewVarHandle", Class.class, java.nio.ByteOrder.class)
                .invoke(null, viewArrayClass,
                        littleEndian ? java.nio.ByteOrder.LITTLE_ENDIAN : java.nio.ByteOrder.BIG_ENDIAN);
    }
    public static java.lang.invoke.VarHandle jitVarByteBufferViewHandle(
            Class<?> viewArrayClass, boolean littleEndian) throws Exception {
        return (java.lang.invoke.VarHandle) java.lang.invoke.MethodHandles.class
                .getMethod("byteBufferViewVarHandle", Class.class, java.nio.ByteOrder.class)
                .invoke(null, viewArrayClass,
                        littleEndian ? java.nio.ByteOrder.LITTLE_ENDIAN : java.nio.ByteOrder.BIG_ENDIAN);
    }
    public static int jitVarByteViewIntGet(java.lang.invoke.VarHandle handle, byte[] array, int index) {
        return (int) handle.get(array, index);
    }
    public static void jitVarByteViewIntSet(java.lang.invoke.VarHandle handle, byte[] array, int index, int value) {
        handle.set(array, index, value);
    }
    public static boolean jitVarByteViewIntCas(java.lang.invoke.VarHandle handle, byte[] array, int index,
            int expected, int value) {
        return handle.compareAndSet(array, index, expected, value);
    }
    public static int jitVarByteViewIntAdd(java.lang.invoke.VarHandle handle, byte[] array, int index, int value) {
        return (int) handle.getAndAdd(array, index, value);
    }
    public static long jitVarByteViewLongGet(java.lang.invoke.VarHandle handle, byte[] array, int index) {
        return (long) handle.get(array, index);
    }
    public static void jitVarByteViewLongSet(java.lang.invoke.VarHandle handle, byte[] array, int index, long value) {
        handle.set(array, index, value);
    }
    public static boolean jitVarByteViewLongCas(java.lang.invoke.VarHandle handle, byte[] array, int index,
            long expected, long value) {
        return handle.compareAndSet(array, index, expected, value);
    }
    public static long jitVarByteViewLongAdd(java.lang.invoke.VarHandle handle, byte[] array, int index, long value) {
        return (long) handle.getAndAdd(array, index, value);
    }
    public static short jitVarByteViewShortGet(java.lang.invoke.VarHandle handle, byte[] array, int index) {
        return (short) handle.get(array, index);
    }
    public static void jitVarByteViewShortSet(java.lang.invoke.VarHandle handle, byte[] array, int index, short value) {
        handle.set(array, index, value);
    }
    public static boolean jitVarByteViewShortCas(java.lang.invoke.VarHandle handle, byte[] array, int index,
            short expected, short value) {
        return handle.compareAndSet(array, index, expected, value);
    }
    public static short jitVarByteViewShortAdd(java.lang.invoke.VarHandle handle, byte[] array, int index, short value) {
        return (short) handle.getAndAdd(array, index, value);
    }
    public static char jitVarByteViewCharGet(java.lang.invoke.VarHandle handle, byte[] array, int index) {
        return (char) handle.get(array, index);
    }
    public static void jitVarByteViewCharSet(java.lang.invoke.VarHandle handle, byte[] array, int index, char value) {
        handle.set(array, index, value);
    }
    public static boolean jitVarByteViewCharCas(java.lang.invoke.VarHandle handle, byte[] array, int index,
            char expected, char value) {
        return handle.compareAndSet(array, index, expected, value);
    }
    public static char jitVarByteViewCharAdd(java.lang.invoke.VarHandle handle, byte[] array, int index, char value) {
        return (char) handle.getAndAdd(array, index, value);
    }
    public static float jitVarByteViewFloatGet(java.lang.invoke.VarHandle handle, byte[] array, int index) {
        return (float) handle.get(array, index);
    }
    public static void jitVarByteViewFloatSet(java.lang.invoke.VarHandle handle, byte[] array, int index, float value) {
        handle.set(array, index, value);
    }
    public static boolean jitVarByteViewFloatCas(java.lang.invoke.VarHandle handle, byte[] array, int index,
            float expected, float value) {
        return handle.compareAndSet(array, index, expected, value);
    }
    public static float jitVarByteViewFloatAdd(java.lang.invoke.VarHandle handle, byte[] array, int index, float value) {
        return (float) handle.getAndAdd(array, index, value);
    }
    public static double jitVarByteViewDoubleGet(java.lang.invoke.VarHandle handle, byte[] array, int index) {
        return (double) handle.get(array, index);
    }
    public static void jitVarByteViewDoubleSet(java.lang.invoke.VarHandle handle, byte[] array, int index, double value) {
        handle.set(array, index, value);
    }
    public static boolean jitVarByteViewDoubleCas(java.lang.invoke.VarHandle handle, byte[] array, int index,
            double expected, double value) {
        return handle.compareAndSet(array, index, expected, value);
    }
    public static double jitVarByteViewDoubleAdd(java.lang.invoke.VarHandle handle, byte[] array, int index,
            double value) {
        return (double) handle.getAndAdd(array, index, value);
    }
    public static int jitVarBufferViewIntGet(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index) {
        return (int) handle.get(buffer, index);
    }
    public static void jitVarBufferViewIntSet(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index, int value) {
        handle.set(buffer, index, value);
    }
    public static boolean jitVarBufferViewIntCas(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index, int expected, int value) {
        return handle.compareAndSet(buffer, index, expected, value);
    }
    public static int jitVarBufferViewIntAdd(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index, int value) {
        return (int) handle.getAndAdd(buffer, index, value);
    }
    public static long jitVarBufferViewLongGet(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index) {
        return (long) handle.get(buffer, index);
    }
    public static void jitVarBufferViewLongSet(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index, long value) {
        handle.set(buffer, index, value);
    }
    public static boolean jitVarBufferViewLongCas(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index, long expected, long value) {
        return handle.compareAndSet(buffer, index, expected, value);
    }
    public static long jitVarBufferViewLongAdd(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index, long value) {
        return (long) handle.getAndAdd(buffer, index, value);
    }
    public static short jitVarBufferViewShortGet(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index) {
        return (short) handle.get(buffer, index);
    }
    public static void jitVarBufferViewShortSet(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index, short value) {
        handle.set(buffer, index, value);
    }
    public static boolean jitVarBufferViewShortCas(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index, short expected, short value) {
        return handle.compareAndSet(buffer, index, expected, value);
    }
    public static short jitVarBufferViewShortAdd(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index, short value) {
        return (short) handle.getAndAdd(buffer, index, value);
    }
    public static char jitVarBufferViewCharGet(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index) {
        return (char) handle.get(buffer, index);
    }
    public static void jitVarBufferViewCharSet(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index, char value) {
        handle.set(buffer, index, value);
    }
    public static boolean jitVarBufferViewCharCas(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index, char expected, char value) {
        return handle.compareAndSet(buffer, index, expected, value);
    }
    public static char jitVarBufferViewCharAdd(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index, char value) {
        return (char) handle.getAndAdd(buffer, index, value);
    }
    public static float jitVarBufferViewFloatGet(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index) {
        return (float) handle.get(buffer, index);
    }
    public static void jitVarBufferViewFloatSet(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index, float value) {
        handle.set(buffer, index, value);
    }
    public static boolean jitVarBufferViewFloatCas(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index, float expected, float value) {
        return handle.compareAndSet(buffer, index, expected, value);
    }
    public static float jitVarBufferViewFloatAdd(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index, float value) {
        return (float) handle.getAndAdd(buffer, index, value);
    }
    public static double jitVarBufferViewDoubleGet(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index) {
        return (double) handle.get(buffer, index);
    }
    public static void jitVarBufferViewDoubleSet(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index, double value) {
        handle.set(buffer, index, value);
    }
    public static boolean jitVarBufferViewDoubleCas(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index, double expected, double value) {
        return handle.compareAndSet(buffer, index, expected, value);
    }
    public static double jitVarBufferViewDoubleAdd(java.lang.invoke.VarHandle handle,
            java.nio.ByteBuffer buffer, int index, double value) {
        return (double) handle.getAndAdd(buffer, index, value);
    }
    public static int jitVarIntArrayGet(java.lang.invoke.VarHandle handle, int[] array, int index) {
        return (int) handle.get(array, index);
    }
    public static void jitVarIntArraySet(java.lang.invoke.VarHandle handle, int[] array, int index, int value) {
        handle.set(array, index, value);
    }
    public static boolean jitVarIntArrayCas(java.lang.invoke.VarHandle handle, int[] array, int index,
            int expected, int value) {
        return handle.compareAndSet(array, index, expected, value);
    }
    public static int jitVarIntArrayAdd(java.lang.invoke.VarHandle handle, int[] array, int index, int value) {
        return (int) handle.getAndAdd(array, index, value);
    }
    public static Object jitVarObjectArrayGet(java.lang.invoke.VarHandle handle, Object[] array, int index) {
        return (Object) handle.get(array, index);
    }
    public static void jitVarObjectArraySet(java.lang.invoke.VarHandle handle, Object[] array, int index,
            Object value) {
        handle.set(array, index, value);
    }
    public static boolean jitVarObjectArrayCas(java.lang.invoke.VarHandle handle, Object[] array, int index,
            Object expected, Object value) {
        return handle.compareAndSet(array, index, expected, value);
    }
    public static Object jitVarObjectArraySwap(java.lang.invoke.VarHandle handle, Object[] array, int index,
            Object value) {
        return (Object) handle.getAndSet(array, index, value);
    }
    public static double jitVarDoubleArrayGet(java.lang.invoke.VarHandle handle, double[] array, int index) {
        return (double) handle.get(array, index);
    }
    public static void jitVarDoubleArraySet(java.lang.invoke.VarHandle handle, double[] array, int index,
            double value) {
        handle.set(array, index, value);
    }
    public static boolean jitVarDoubleArrayCas(java.lang.invoke.VarHandle handle, double[] array, int index,
            double expected, double value) {
        return handle.compareAndSet(array, index, expected, value);
    }
    public static double jitVarDoubleArrayAdd(java.lang.invoke.VarHandle handle, double[] array, int index,
            double value) {
        return (double) handle.getAndAdd(array, index, value);
    }
    public static float jitVarFloatArrayGet(java.lang.invoke.VarHandle handle, float[] array, int index) {
        return (float) handle.get(array, index);
    }
    public static void jitVarFloatArraySet(java.lang.invoke.VarHandle handle, float[] array, int index,
            float value) {
        handle.set(array, index, value);
    }
    public static boolean jitVarFloatArrayCas(java.lang.invoke.VarHandle handle, float[] array, int index,
            float expected, float value) {
        return handle.compareAndSet(array, index, expected, value);
    }
    public static float jitVarFloatArrayAdd(java.lang.invoke.VarHandle handle, float[] array, int index,
            float value) {
        return (float) handle.getAndAdd(array, index, value);
    }
    public static byte jitVarByteArrayGet(java.lang.invoke.VarHandle handle, byte[] array, int index) {
        return (byte) handle.get(array, index);
    }
    public static void jitVarByteArraySet(java.lang.invoke.VarHandle handle, byte[] array, int index, byte value) {
        handle.set(array, index, value);
    }
    public static boolean jitVarByteArrayCas(java.lang.invoke.VarHandle handle, byte[] array, int index,
            byte expected, byte value) {
        return handle.compareAndSet(array, index, expected, value);
    }
    public static byte jitVarByteArrayAdd(java.lang.invoke.VarHandle handle, byte[] array, int index, byte value) {
        return (byte) handle.getAndAdd(array, index, value);
    }
    public static boolean jitVarBooleanArrayGet(java.lang.invoke.VarHandle handle, boolean[] array, int index) {
        return (boolean) handle.get(array, index);
    }
    public static void jitVarBooleanArraySet(java.lang.invoke.VarHandle handle, boolean[] array, int index,
            boolean value) {
        handle.set(array, index, value);
    }
    public static boolean jitVarBooleanArrayCas(java.lang.invoke.VarHandle handle, boolean[] array, int index,
            boolean expected, boolean value) {
        return handle.compareAndSet(array, index, expected, value);
    }
    public static boolean jitVarBooleanArrayXor(java.lang.invoke.VarHandle handle, boolean[] array, int index,
            boolean value) {
        return (boolean) handle.getAndBitwiseXor(array, index, value);
    }
    public static short jitVarShortArrayGet(java.lang.invoke.VarHandle handle, short[] array, int index) {
        return (short) handle.get(array, index);
    }
    public static void jitVarShortArraySet(java.lang.invoke.VarHandle handle, short[] array, int index, short value) {
        handle.set(array, index, value);
    }
    public static boolean jitVarShortArrayCas(java.lang.invoke.VarHandle handle, short[] array, int index,
            short expected, short value) {
        return handle.compareAndSet(array, index, expected, value);
    }
    public static short jitVarShortArrayAdd(java.lang.invoke.VarHandle handle, short[] array, int index, short value) {
        return (short) handle.getAndAdd(array, index, value);
    }
    public static char jitVarCharArrayGet(java.lang.invoke.VarHandle handle, char[] array, int index) {
        return (char) handle.get(array, index);
    }
    public static void jitVarCharArraySet(java.lang.invoke.VarHandle handle, char[] array, int index, char value) {
        handle.set(array, index, value);
    }
    public static boolean jitVarCharArrayCas(java.lang.invoke.VarHandle handle, char[] array, int index,
            char expected, char value) {
        return handle.compareAndSet(array, index, expected, value);
    }
    public static char jitVarCharArrayAdd(java.lang.invoke.VarHandle handle, char[] array, int index, char value) {
        return (char) handle.getAndAdd(array, index, value);
    }
    public static java.lang.invoke.VarHandle jitVarReferenceHandle() throws Exception {
        return (java.lang.invoke.VarHandle) java.lang.invoke.MethodHandles.Lookup.class
                .getMethod("findVarHandle", Class.class, String.class, Class.class)
                .invoke(java.lang.invoke.MethodHandles.lookup(), Hello.class, "jitVarReference", Object.class);
    }
    public static Object jitVarReferenceGet(java.lang.invoke.VarHandle handle, Hello receiver) {
        return (Object) handle.get(receiver);
    }
    public static void jitVarReferenceSet(java.lang.invoke.VarHandle handle, Hello receiver, Object value) {
        handle.set(receiver, value);
    }
    public static boolean jitVarReferenceCas(java.lang.invoke.VarHandle handle, Hello receiver,
            Object expected, Object value) {
        return handle.compareAndSet(receiver, expected, value);
    }
    public static Object jitVarReferenceExchange(java.lang.invoke.VarHandle handle, Hello receiver,
            Object expected, Object value) {
        return (Object) handle.compareAndExchange(receiver, expected, value);
    }
    public static Object jitVarReferenceSwap(java.lang.invoke.VarHandle handle, Hello receiver, Object value) {
        return (Object) handle.getAndSet(receiver, value);
    }
    public static Object jitVarReferenceGetOpaque(java.lang.invoke.VarHandle h, Hello r) { return (Object) h.getOpaque(r); }
    public static Object jitVarReferenceGetAcquire(java.lang.invoke.VarHandle h, Hello r) { return (Object) h.getAcquire(r); }
    public static Object jitVarReferenceGetVolatile(java.lang.invoke.VarHandle h, Hello r) { return (Object) h.getVolatile(r); }
    public static void jitVarReferenceSetOpaque(java.lang.invoke.VarHandle h, Hello r, Object v) { h.setOpaque(r, v); }
    public static void jitVarReferenceSetRelease(java.lang.invoke.VarHandle h, Hello r, Object v) { h.setRelease(r, v); }
    public static void jitVarReferenceSetVolatile(java.lang.invoke.VarHandle h, Hello r, Object v) { h.setVolatile(r, v); }
    public static boolean jitVarReferenceWeakPlain(java.lang.invoke.VarHandle h, Hello r, Object e, Object v) { return h.weakCompareAndSetPlain(r, e, v); }
    public static boolean jitVarReferenceWeakAcquire(java.lang.invoke.VarHandle h, Hello r, Object e, Object v) { return h.weakCompareAndSetAcquire(r, e, v); }
    public static boolean jitVarReferenceWeakRelease(java.lang.invoke.VarHandle h, Hello r, Object e, Object v) { return h.weakCompareAndSetRelease(r, e, v); }
    public static boolean jitVarReferenceWeakVolatile(java.lang.invoke.VarHandle h, Hello r, Object e, Object v) { return h.weakCompareAndSet(r, e, v); }
    public static Object jitVarReferenceExchangeAcquire(java.lang.invoke.VarHandle h, Hello r, Object e, Object v) { return (Object) h.compareAndExchangeAcquire(r, e, v); }
    public static Object jitVarReferenceExchangeRelease(java.lang.invoke.VarHandle h, Hello r, Object e, Object v) { return (Object) h.compareAndExchangeRelease(r, e, v); }
    public static Object jitVarReferenceSwapAcquire(java.lang.invoke.VarHandle h, Hello r, Object v) { return (Object) h.getAndSetAcquire(r, v); }
    public static Object jitVarReferenceSwapRelease(java.lang.invoke.VarHandle h, Hello r, Object v) { return (Object) h.getAndSetRelease(r, v); }
    public static java.lang.invoke.VarHandle jitVarStaticReferenceHandle() throws Exception {
        return (java.lang.invoke.VarHandle) java.lang.invoke.MethodHandles.Lookup.class
                .getMethod("findStaticVarHandle", Class.class, String.class, Class.class)
                .invoke(java.lang.invoke.MethodHandles.lookup(), Hello.class, "jitVarStaticReference", Object.class);
    }
    public static Object jitVarStaticReferenceGetOpaque(java.lang.invoke.VarHandle h) { return (Object) h.getOpaque(); }
    public static Object jitVarStaticReferenceGetAcquire(java.lang.invoke.VarHandle h) { return (Object) h.getAcquire(); }
    public static Object jitVarStaticReferenceGetVolatile(java.lang.invoke.VarHandle h) { return (Object) h.getVolatile(); }
    public static void jitVarStaticReferenceSetOpaque(java.lang.invoke.VarHandle h, Object v) { h.setOpaque(v); }
    public static void jitVarStaticReferenceSetRelease(java.lang.invoke.VarHandle h, Object v) { h.setRelease(v); }
    public static void jitVarStaticReferenceSetVolatile(java.lang.invoke.VarHandle h, Object v) { h.setVolatile(v); }
    public static boolean jitVarStaticReferenceWeakPlain(java.lang.invoke.VarHandle h, Object e, Object v) { return h.weakCompareAndSetPlain(e, v); }
    public static boolean jitVarStaticReferenceWeakAcquire(java.lang.invoke.VarHandle h, Object e, Object v) { return h.weakCompareAndSetAcquire(e, v); }
    public static boolean jitVarStaticReferenceWeakRelease(java.lang.invoke.VarHandle h, Object e, Object v) { return h.weakCompareAndSetRelease(e, v); }
    public static boolean jitVarStaticReferenceWeakVolatile(java.lang.invoke.VarHandle h, Object e, Object v) { return h.weakCompareAndSet(e, v); }
    public static Object jitVarStaticReferenceExchangeAcquire(java.lang.invoke.VarHandle h, Object e, Object v) { return (Object) h.compareAndExchangeAcquire(e, v); }
    public static Object jitVarStaticReferenceExchangeRelease(java.lang.invoke.VarHandle h, Object e, Object v) { return (Object) h.compareAndExchangeRelease(e, v); }
    public static Object jitVarStaticReferenceSwapAcquire(java.lang.invoke.VarHandle h, Object v) { return (Object) h.getAndSetAcquire(v); }
    public static Object jitVarStaticReferenceSwapRelease(java.lang.invoke.VarHandle h, Object v) { return (Object) h.getAndSetRelease(v); }
    public static Object jitVarObjectArrayGetOpaque(java.lang.invoke.VarHandle h, Object[] a, int i) { return (Object) h.getOpaque(a, i); }
    public static Object jitVarObjectArrayGetAcquire(java.lang.invoke.VarHandle h, Object[] a, int i) { return (Object) h.getAcquire(a, i); }
    public static Object jitVarObjectArrayGetVolatile(java.lang.invoke.VarHandle h, Object[] a, int i) { return (Object) h.getVolatile(a, i); }
    public static void jitVarObjectArraySetOpaque(java.lang.invoke.VarHandle h, Object[] a, int i, Object v) { h.setOpaque(a, i, v); }
    public static void jitVarObjectArraySetRelease(java.lang.invoke.VarHandle h, Object[] a, int i, Object v) { h.setRelease(a, i, v); }
    public static void jitVarObjectArraySetVolatile(java.lang.invoke.VarHandle h, Object[] a, int i, Object v) { h.setVolatile(a, i, v); }
    public static boolean jitVarObjectArrayWeakPlain(java.lang.invoke.VarHandle h, Object[] a, int i, Object e, Object v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarObjectArrayWeakAcquire(java.lang.invoke.VarHandle h, Object[] a, int i, Object e, Object v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarObjectArrayWeakRelease(java.lang.invoke.VarHandle h, Object[] a, int i, Object e, Object v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarObjectArrayWeakVolatile(java.lang.invoke.VarHandle h, Object[] a, int i, Object e, Object v) { return h.weakCompareAndSet(a, i, e, v); }
    public static Object jitVarObjectArrayExchangeAcquire(java.lang.invoke.VarHandle h, Object[] a, int i, Object e, Object v) { return (Object) h.compareAndExchangeAcquire(a, i, e, v); }
    public static Object jitVarObjectArrayExchangeRelease(java.lang.invoke.VarHandle h, Object[] a, int i, Object e, Object v) { return (Object) h.compareAndExchangeRelease(a, i, e, v); }
    public static Object jitVarObjectArraySwapAcquire(java.lang.invoke.VarHandle h, Object[] a, int i, Object v) { return (Object) h.getAndSetAcquire(a, i, v); }
    public static Object jitVarObjectArraySwapRelease(java.lang.invoke.VarHandle h, Object[] a, int i, Object v) { return (Object) h.getAndSetRelease(a, i, v); }
    public static void jitVarPublish(java.lang.invoke.VarHandle handle, Hello receiver, int value, boolean sequential) {
        receiver.jitVarPayload = value;
        if (sequential) handle.setVolatile(receiver, value);
        else handle.setRelease(receiver, value);
    }
    public static int jitVarObserve(java.lang.invoke.VarHandle handle, Hello receiver, int expected, boolean sequential) {
        int observed = sequential ? (int) handle.getVolatile(receiver) : (int) handle.getAcquire(receiver);
        return observed == expected ? receiver.jitVarPayload : -1;
    }
    public static java.lang.invoke.VarHandle jitVarHandle() throws Exception {
        // The fixture compiler exposes Java 8 Lookup; the pinned Android runtime has this API.
        return (java.lang.invoke.VarHandle) java.lang.invoke.MethodHandles.Lookup.class
                .getMethod("findVarHandle", Class.class, String.class, Class.class)
                .invoke(java.lang.invoke.MethodHandles.lookup(), Hello.class, "jitVarValue", int.class);
    }
    public static int jitVarGet(java.lang.invoke.VarHandle handle, Hello receiver) {
        return (int) handle.get(receiver);
    }
    public static void jitVarSet(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        handle.set(receiver, value);
    }
    public static boolean jitVarCas(java.lang.invoke.VarHandle handle, Hello receiver, int expected, int value) {
        return handle.compareAndSet(receiver, expected, value);
    }
    public static int jitVarExchange(java.lang.invoke.VarHandle handle, Hello receiver, int expected, int value) {
        return (int) handle.compareAndExchange(receiver, expected, value);
    }
    public static int jitVarAdd(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        return (int) handle.getAndAdd(receiver, value);
    }
    public static int jitVarSwap(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        return (int) handle.getAndSet(receiver, value);
    }
    public static int jitVarXor(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        return (int) handle.getAndBitwiseXor(receiver, value);
    }
    public static int jitVarIntGetOpaque(java.lang.invoke.VarHandle handle, Hello receiver) {
        return (int) handle.getOpaque(receiver);
    }
    public static int jitVarIntGetAcquire(java.lang.invoke.VarHandle handle, Hello receiver) {
        return (int) handle.getAcquire(receiver);
    }
    public static int jitVarIntGetVolatile(java.lang.invoke.VarHandle handle, Hello receiver) {
        return (int) handle.getVolatile(receiver);
    }
    public static void jitVarIntSetOpaque(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        handle.setOpaque(receiver, value);
    }
    public static void jitVarIntSetRelease(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        handle.setRelease(receiver, value);
    }
    public static void jitVarIntSetVolatile(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        handle.setVolatile(receiver, value);
    }
    public static boolean jitVarIntWeakPlain(java.lang.invoke.VarHandle handle, Hello receiver,
            int expected, int value) {
        return handle.weakCompareAndSetPlain(receiver, expected, value);
    }
    public static boolean jitVarIntWeakAcquire(java.lang.invoke.VarHandle handle, Hello receiver,
            int expected, int value) {
        return handle.weakCompareAndSetAcquire(receiver, expected, value);
    }
    public static boolean jitVarIntWeakRelease(java.lang.invoke.VarHandle handle, Hello receiver,
            int expected, int value) {
        return handle.weakCompareAndSetRelease(receiver, expected, value);
    }
    public static boolean jitVarIntWeakVolatile(java.lang.invoke.VarHandle handle, Hello receiver,
            int expected, int value) {
        return handle.weakCompareAndSet(receiver, expected, value);
    }
    public static int jitVarIntExchangeAcquire(java.lang.invoke.VarHandle handle, Hello receiver,
            int expected, int value) {
        return (int) handle.compareAndExchangeAcquire(receiver, expected, value);
    }
    public static int jitVarIntExchangeRelease(java.lang.invoke.VarHandle handle, Hello receiver,
            int expected, int value) {
        return (int) handle.compareAndExchangeRelease(receiver, expected, value);
    }
    public static int jitVarIntSwapAcquire(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        return (int) handle.getAndSetAcquire(receiver, value);
    }
    public static int jitVarIntSwapRelease(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        return (int) handle.getAndSetRelease(receiver, value);
    }
    public static int jitVarIntAddAcquire(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        return (int) handle.getAndAddAcquire(receiver, value);
    }
    public static int jitVarIntAddRelease(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        return (int) handle.getAndAddRelease(receiver, value);
    }
    public static int jitVarIntOr(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        return (int) handle.getAndBitwiseOr(receiver, value);
    }
    public static int jitVarIntOrAcquire(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        return (int) handle.getAndBitwiseOrAcquire(receiver, value);
    }
    public static int jitVarIntOrRelease(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        return (int) handle.getAndBitwiseOrRelease(receiver, value);
    }
    public static int jitVarIntAnd(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        return (int) handle.getAndBitwiseAnd(receiver, value);
    }
    public static int jitVarIntAndAcquire(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        return (int) handle.getAndBitwiseAndAcquire(receiver, value);
    }
    public static int jitVarIntAndRelease(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        return (int) handle.getAndBitwiseAndRelease(receiver, value);
    }
    public static int jitVarIntXorAcquire(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        return (int) handle.getAndBitwiseXorAcquire(receiver, value);
    }
    public static int jitVarIntXorRelease(java.lang.invoke.VarHandle handle, Hello receiver, int value) {
        return (int) handle.getAndBitwiseXorRelease(receiver, value);
    }
    public static java.lang.invoke.VarHandle jitVarStaticHandle() throws Exception {
        return (java.lang.invoke.VarHandle) java.lang.invoke.MethodHandles.Lookup.class
                .getMethod("findStaticVarHandle", Class.class, String.class, Class.class)
                .invoke(java.lang.invoke.MethodHandles.lookup(), Hello.class, "jitVarStaticValue", int.class);
    }
    public static int jitVarStaticIntGetOpaque(java.lang.invoke.VarHandle h) { return (int) h.getOpaque(); }
    public static int jitVarStaticIntGetAcquire(java.lang.invoke.VarHandle h) { return (int) h.getAcquire(); }
    public static int jitVarStaticIntGetVolatile(java.lang.invoke.VarHandle h) { return (int) h.getVolatile(); }
    public static void jitVarStaticIntSetOpaque(java.lang.invoke.VarHandle h, int v) { h.setOpaque(v); }
    public static void jitVarStaticIntSetRelease(java.lang.invoke.VarHandle h, int v) { h.setRelease(v); }
    public static void jitVarStaticIntSetVolatile(java.lang.invoke.VarHandle h, int v) { h.setVolatile(v); }
    public static boolean jitVarStaticIntWeakPlain(java.lang.invoke.VarHandle h, int e, int v) { return h.weakCompareAndSetPlain(e, v); }
    public static boolean jitVarStaticIntWeakAcquire(java.lang.invoke.VarHandle h, int e, int v) { return h.weakCompareAndSetAcquire(e, v); }
    public static boolean jitVarStaticIntWeakRelease(java.lang.invoke.VarHandle h, int e, int v) { return h.weakCompareAndSetRelease(e, v); }
    public static boolean jitVarStaticIntWeakVolatile(java.lang.invoke.VarHandle h, int e, int v) { return h.weakCompareAndSet(e, v); }
    public static int jitVarStaticIntExchangeAcquire(java.lang.invoke.VarHandle h, int e, int v) { return (int) h.compareAndExchangeAcquire(e, v); }
    public static int jitVarStaticIntExchangeRelease(java.lang.invoke.VarHandle h, int e, int v) { return (int) h.compareAndExchangeRelease(e, v); }
    public static int jitVarStaticIntSwapAcquire(java.lang.invoke.VarHandle h, int v) { return (int) h.getAndSetAcquire(v); }
    public static int jitVarStaticIntSwapRelease(java.lang.invoke.VarHandle h, int v) { return (int) h.getAndSetRelease(v); }
    public static int jitVarStaticIntAddAcquire(java.lang.invoke.VarHandle h, int v) { return (int) h.getAndAddAcquire(v); }
    public static int jitVarStaticIntAddRelease(java.lang.invoke.VarHandle h, int v) { return (int) h.getAndAddRelease(v); }
    public static int jitVarStaticIntOr(java.lang.invoke.VarHandle h, int v) { return (int) h.getAndBitwiseOr(v); }
    public static int jitVarStaticIntOrAcquire(java.lang.invoke.VarHandle h, int v) { return (int) h.getAndBitwiseOrAcquire(v); }
    public static int jitVarStaticIntOrRelease(java.lang.invoke.VarHandle h, int v) { return (int) h.getAndBitwiseOrRelease(v); }
    public static int jitVarStaticIntAnd(java.lang.invoke.VarHandle h, int v) { return (int) h.getAndBitwiseAnd(v); }
    public static int jitVarStaticIntAndAcquire(java.lang.invoke.VarHandle h, int v) { return (int) h.getAndBitwiseAndAcquire(v); }
    public static int jitVarStaticIntAndRelease(java.lang.invoke.VarHandle h, int v) { return (int) h.getAndBitwiseAndRelease(v); }
    public static int jitVarStaticIntXorAcquire(java.lang.invoke.VarHandle h, int v) { return (int) h.getAndBitwiseXorAcquire(v); }
    public static int jitVarStaticIntXorRelease(java.lang.invoke.VarHandle h, int v) { return (int) h.getAndBitwiseXorRelease(v); }
    public static int jitVarIntArrayGetOpaque(java.lang.invoke.VarHandle h, int[] a, int i) { return (int) h.getOpaque(a, i); }
    public static int jitVarIntArrayGetAcquire(java.lang.invoke.VarHandle h, int[] a, int i) { return (int) h.getAcquire(a, i); }
    public static int jitVarIntArrayGetVolatile(java.lang.invoke.VarHandle h, int[] a, int i) { return (int) h.getVolatile(a, i); }
    public static void jitVarIntArraySetOpaque(java.lang.invoke.VarHandle h, int[] a, int i, int v) { h.setOpaque(a, i, v); }
    public static void jitVarIntArraySetRelease(java.lang.invoke.VarHandle h, int[] a, int i, int v) { h.setRelease(a, i, v); }
    public static void jitVarIntArraySetVolatile(java.lang.invoke.VarHandle h, int[] a, int i, int v) { h.setVolatile(a, i, v); }
    public static boolean jitVarIntArrayWeakPlain(java.lang.invoke.VarHandle h, int[] a, int i, int e, int v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarIntArrayWeakAcquire(java.lang.invoke.VarHandle h, int[] a, int i, int e, int v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarIntArrayWeakRelease(java.lang.invoke.VarHandle h, int[] a, int i, int e, int v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarIntArrayWeakVolatile(java.lang.invoke.VarHandle h, int[] a, int i, int e, int v) { return h.weakCompareAndSet(a, i, e, v); }
    public static int jitVarIntArrayExchangeAcquire(java.lang.invoke.VarHandle h, int[] a, int i, int e, int v) { return (int) h.compareAndExchangeAcquire(a, i, e, v); }
    public static int jitVarIntArrayExchangeRelease(java.lang.invoke.VarHandle h, int[] a, int i, int e, int v) { return (int) h.compareAndExchangeRelease(a, i, e, v); }
    public static int jitVarIntArraySwapAcquire(java.lang.invoke.VarHandle h, int[] a, int i, int v) { return (int) h.getAndSetAcquire(a, i, v); }
    public static int jitVarIntArraySwapRelease(java.lang.invoke.VarHandle h, int[] a, int i, int v) { return (int) h.getAndSetRelease(a, i, v); }
    public static int jitVarIntArrayAddAcquire(java.lang.invoke.VarHandle h, int[] a, int i, int v) { return (int) h.getAndAddAcquire(a, i, v); }
    public static int jitVarIntArrayAddRelease(java.lang.invoke.VarHandle h, int[] a, int i, int v) { return (int) h.getAndAddRelease(a, i, v); }
    public static int jitVarIntArrayOr(java.lang.invoke.VarHandle h, int[] a, int i, int v) { return (int) h.getAndBitwiseOr(a, i, v); }
    public static int jitVarIntArrayOrAcquire(java.lang.invoke.VarHandle h, int[] a, int i, int v) { return (int) h.getAndBitwiseOrAcquire(a, i, v); }
    public static int jitVarIntArrayOrRelease(java.lang.invoke.VarHandle h, int[] a, int i, int v) { return (int) h.getAndBitwiseOrRelease(a, i, v); }
    public static int jitVarIntArrayAnd(java.lang.invoke.VarHandle h, int[] a, int i, int v) { return (int) h.getAndBitwiseAnd(a, i, v); }
    public static int jitVarIntArrayAndAcquire(java.lang.invoke.VarHandle h, int[] a, int i, int v) { return (int) h.getAndBitwiseAndAcquire(a, i, v); }
    public static int jitVarIntArrayAndRelease(java.lang.invoke.VarHandle h, int[] a, int i, int v) { return (int) h.getAndBitwiseAndRelease(a, i, v); }
    public static int jitVarIntArrayXorAcquire(java.lang.invoke.VarHandle h, int[] a, int i, int v) { return (int) h.getAndBitwiseXorAcquire(a, i, v); }
    public static int jitVarIntArrayXorRelease(java.lang.invoke.VarHandle h, int[] a, int i, int v) { return (int) h.getAndBitwiseXorRelease(a, i, v); }
    public static int jitVarByteViewIntGetOpaque(java.lang.invoke.VarHandle h, byte[] a, int i) { return (int) h.getOpaque(a, i); }
    public static int jitVarByteViewIntGetAcquire(java.lang.invoke.VarHandle h, byte[] a, int i) { return (int) h.getAcquire(a, i); }
    public static int jitVarByteViewIntGetVolatile(java.lang.invoke.VarHandle h, byte[] a, int i) { return (int) h.getVolatile(a, i); }
    public static void jitVarByteViewIntSetOpaque(java.lang.invoke.VarHandle h, byte[] a, int i, int v) { h.setOpaque(a, i, v); }
    public static void jitVarByteViewIntSetRelease(java.lang.invoke.VarHandle h, byte[] a, int i, int v) { h.setRelease(a, i, v); }
    public static void jitVarByteViewIntSetVolatile(java.lang.invoke.VarHandle h, byte[] a, int i, int v) { h.setVolatile(a, i, v); }
    public static boolean jitVarByteViewIntWeakPlain(java.lang.invoke.VarHandle h, byte[] a, int i, int e, int v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarByteViewIntWeakAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, int e, int v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarByteViewIntWeakRelease(java.lang.invoke.VarHandle h, byte[] a, int i, int e, int v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarByteViewIntWeakVolatile(java.lang.invoke.VarHandle h, byte[] a, int i, int e, int v) { return h.weakCompareAndSet(a, i, e, v); }
    public static int jitVarByteViewIntExchangeAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, int e, int v) { return (int) h.compareAndExchangeAcquire(a, i, e, v); }
    public static int jitVarByteViewIntExchangeRelease(java.lang.invoke.VarHandle h, byte[] a, int i, int e, int v) { return (int) h.compareAndExchangeRelease(a, i, e, v); }
    public static int jitVarByteViewIntSwapAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, int v) { return (int) h.getAndSetAcquire(a, i, v); }
    public static int jitVarByteViewIntSwapRelease(java.lang.invoke.VarHandle h, byte[] a, int i, int v) { return (int) h.getAndSetRelease(a, i, v); }
    public static int jitVarByteViewIntAddAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, int v) { return (int) h.getAndAddAcquire(a, i, v); }
    public static int jitVarByteViewIntAddRelease(java.lang.invoke.VarHandle h, byte[] a, int i, int v) { return (int) h.getAndAddRelease(a, i, v); }
    public static int jitVarByteViewIntOr(java.lang.invoke.VarHandle h, byte[] a, int i, int v) { return (int) h.getAndBitwiseOr(a, i, v); }
    public static int jitVarByteViewIntOrAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, int v) { return (int) h.getAndBitwiseOrAcquire(a, i, v); }
    public static int jitVarByteViewIntOrRelease(java.lang.invoke.VarHandle h, byte[] a, int i, int v) { return (int) h.getAndBitwiseOrRelease(a, i, v); }
    public static int jitVarByteViewIntAnd(java.lang.invoke.VarHandle h, byte[] a, int i, int v) { return (int) h.getAndBitwiseAnd(a, i, v); }
    public static int jitVarByteViewIntAndAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, int v) { return (int) h.getAndBitwiseAndAcquire(a, i, v); }
    public static int jitVarByteViewIntAndRelease(java.lang.invoke.VarHandle h, byte[] a, int i, int v) { return (int) h.getAndBitwiseAndRelease(a, i, v); }
    public static int jitVarByteViewIntXorAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, int v) { return (int) h.getAndBitwiseXorAcquire(a, i, v); }
    public static int jitVarByteViewIntXorRelease(java.lang.invoke.VarHandle h, byte[] a, int i, int v) { return (int) h.getAndBitwiseXorRelease(a, i, v); }
    public static long jitVarByteViewLongGetOpaque(java.lang.invoke.VarHandle h, byte[] a, int i) { return (long) h.getOpaque(a, i); }
    public static long jitVarByteViewLongGetAcquire(java.lang.invoke.VarHandle h, byte[] a, int i) { return (long) h.getAcquire(a, i); }
    public static long jitVarByteViewLongGetVolatile(java.lang.invoke.VarHandle h, byte[] a, int i) { return (long) h.getVolatile(a, i); }
    public static void jitVarByteViewLongSetOpaque(java.lang.invoke.VarHandle h, byte[] a, int i, long v) { h.setOpaque(a, i, v); }
    public static void jitVarByteViewLongSetRelease(java.lang.invoke.VarHandle h, byte[] a, int i, long v) { h.setRelease(a, i, v); }
    public static void jitVarByteViewLongSetVolatile(java.lang.invoke.VarHandle h, byte[] a, int i, long v) { h.setVolatile(a, i, v); }
    public static boolean jitVarByteViewLongWeakPlain(java.lang.invoke.VarHandle h, byte[] a, int i, long e, long v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarByteViewLongWeakAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, long e, long v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarByteViewLongWeakRelease(java.lang.invoke.VarHandle h, byte[] a, int i, long e, long v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarByteViewLongWeakVolatile(java.lang.invoke.VarHandle h, byte[] a, int i, long e, long v) { return h.weakCompareAndSet(a, i, e, v); }
    public static long jitVarByteViewLongExchangeAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, long e, long v) { return (long) h.compareAndExchangeAcquire(a, i, e, v); }
    public static long jitVarByteViewLongExchangeRelease(java.lang.invoke.VarHandle h, byte[] a, int i, long e, long v) { return (long) h.compareAndExchangeRelease(a, i, e, v); }
    public static long jitVarByteViewLongSwapAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, long v) { return (long) h.getAndSetAcquire(a, i, v); }
    public static long jitVarByteViewLongSwapRelease(java.lang.invoke.VarHandle h, byte[] a, int i, long v) { return (long) h.getAndSetRelease(a, i, v); }
    public static long jitVarByteViewLongAddAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, long v) { return (long) h.getAndAddAcquire(a, i, v); }
    public static long jitVarByteViewLongAddRelease(java.lang.invoke.VarHandle h, byte[] a, int i, long v) { return (long) h.getAndAddRelease(a, i, v); }
    public static long jitVarByteViewLongOr(java.lang.invoke.VarHandle h, byte[] a, int i, long v) { return (long) h.getAndBitwiseOr(a, i, v); }
    public static long jitVarByteViewLongOrAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, long v) { return (long) h.getAndBitwiseOrAcquire(a, i, v); }
    public static long jitVarByteViewLongOrRelease(java.lang.invoke.VarHandle h, byte[] a, int i, long v) { return (long) h.getAndBitwiseOrRelease(a, i, v); }
    public static long jitVarByteViewLongAnd(java.lang.invoke.VarHandle h, byte[] a, int i, long v) { return (long) h.getAndBitwiseAnd(a, i, v); }
    public static long jitVarByteViewLongAndAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, long v) { return (long) h.getAndBitwiseAndAcquire(a, i, v); }
    public static long jitVarByteViewLongAndRelease(java.lang.invoke.VarHandle h, byte[] a, int i, long v) { return (long) h.getAndBitwiseAndRelease(a, i, v); }
    public static long jitVarByteViewLongXorAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, long v) { return (long) h.getAndBitwiseXorAcquire(a, i, v); }
    public static long jitVarByteViewLongXorRelease(java.lang.invoke.VarHandle h, byte[] a, int i, long v) { return (long) h.getAndBitwiseXorRelease(a, i, v); }
    public static int jitVarBufferViewIntGetOpaque(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i) { return (int) h.getOpaque(a, i); }
    public static int jitVarBufferViewIntGetAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i) { return (int) h.getAcquire(a, i); }
    public static int jitVarBufferViewIntGetVolatile(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i) { return (int) h.getVolatile(a, i); }
    public static void jitVarBufferViewIntSetOpaque(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int v) { h.setOpaque(a, i, v); }
    public static void jitVarBufferViewIntSetRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int v) { h.setRelease(a, i, v); }
    public static void jitVarBufferViewIntSetVolatile(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int v) { h.setVolatile(a, i, v); }
    public static boolean jitVarBufferViewIntWeakPlain(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int e, int v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarBufferViewIntWeakAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int e, int v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarBufferViewIntWeakRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int e, int v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarBufferViewIntWeakVolatile(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int e, int v) { return h.weakCompareAndSet(a, i, e, v); }
    public static int jitVarBufferViewIntExchangeAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int e, int v) { return (int) h.compareAndExchangeAcquire(a, i, e, v); }
    public static int jitVarBufferViewIntExchangeRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int e, int v) { return (int) h.compareAndExchangeRelease(a, i, e, v); }
    public static int jitVarBufferViewIntSwapAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int v) { return (int) h.getAndSetAcquire(a, i, v); }
    public static int jitVarBufferViewIntSwapRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int v) { return (int) h.getAndSetRelease(a, i, v); }
    public static int jitVarBufferViewIntAddAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int v) { return (int) h.getAndAddAcquire(a, i, v); }
    public static int jitVarBufferViewIntAddRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int v) { return (int) h.getAndAddRelease(a, i, v); }
    public static int jitVarBufferViewIntOr(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int v) { return (int) h.getAndBitwiseOr(a, i, v); }
    public static int jitVarBufferViewIntOrAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int v) { return (int) h.getAndBitwiseOrAcquire(a, i, v); }
    public static int jitVarBufferViewIntOrRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int v) { return (int) h.getAndBitwiseOrRelease(a, i, v); }
    public static int jitVarBufferViewIntAnd(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int v) { return (int) h.getAndBitwiseAnd(a, i, v); }
    public static int jitVarBufferViewIntAndAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int v) { return (int) h.getAndBitwiseAndAcquire(a, i, v); }
    public static int jitVarBufferViewIntAndRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int v) { return (int) h.getAndBitwiseAndRelease(a, i, v); }
    public static int jitVarBufferViewIntXorAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int v) { return (int) h.getAndBitwiseXorAcquire(a, i, v); }
    public static int jitVarBufferViewIntXorRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, int v) { return (int) h.getAndBitwiseXorRelease(a, i, v); }
    public static long jitVarBufferViewLongGetOpaque(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i) { return (long) h.getOpaque(a, i); }
    public static long jitVarBufferViewLongGetAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i) { return (long) h.getAcquire(a, i); }
    public static long jitVarBufferViewLongGetVolatile(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i) { return (long) h.getVolatile(a, i); }
    public static void jitVarBufferViewLongSetOpaque(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long v) { h.setOpaque(a, i, v); }
    public static void jitVarBufferViewLongSetRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long v) { h.setRelease(a, i, v); }
    public static void jitVarBufferViewLongSetVolatile(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long v) { h.setVolatile(a, i, v); }
    public static boolean jitVarBufferViewLongWeakPlain(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long e, long v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarBufferViewLongWeakAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long e, long v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarBufferViewLongWeakRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long e, long v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarBufferViewLongWeakVolatile(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long e, long v) { return h.weakCompareAndSet(a, i, e, v); }
    public static long jitVarBufferViewLongExchangeAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long e, long v) { return (long) h.compareAndExchangeAcquire(a, i, e, v); }
    public static long jitVarBufferViewLongExchangeRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long e, long v) { return (long) h.compareAndExchangeRelease(a, i, e, v); }
    public static long jitVarBufferViewLongSwapAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long v) { return (long) h.getAndSetAcquire(a, i, v); }
    public static long jitVarBufferViewLongSwapRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long v) { return (long) h.getAndSetRelease(a, i, v); }
    public static long jitVarBufferViewLongAddAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long v) { return (long) h.getAndAddAcquire(a, i, v); }
    public static long jitVarBufferViewLongAddRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long v) { return (long) h.getAndAddRelease(a, i, v); }
    public static long jitVarBufferViewLongOr(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long v) { return (long) h.getAndBitwiseOr(a, i, v); }
    public static long jitVarBufferViewLongOrAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long v) { return (long) h.getAndBitwiseOrAcquire(a, i, v); }
    public static long jitVarBufferViewLongOrRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long v) { return (long) h.getAndBitwiseOrRelease(a, i, v); }
    public static long jitVarBufferViewLongAnd(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long v) { return (long) h.getAndBitwiseAnd(a, i, v); }
    public static long jitVarBufferViewLongAndAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long v) { return (long) h.getAndBitwiseAndAcquire(a, i, v); }
    public static long jitVarBufferViewLongAndRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long v) { return (long) h.getAndBitwiseAndRelease(a, i, v); }
    public static long jitVarBufferViewLongXorAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long v) { return (long) h.getAndBitwiseXorAcquire(a, i, v); }
    public static long jitVarBufferViewLongXorRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, long v) { return (long) h.getAndBitwiseXorRelease(a, i, v); }
    public static short jitVarByteViewShortGetOpaque(java.lang.invoke.VarHandle h, byte[] a, int i) { return (short) h.getOpaque(a, i); }
    public static short jitVarByteViewShortGetAcquire(java.lang.invoke.VarHandle h, byte[] a, int i) { return (short) h.getAcquire(a, i); }
    public static short jitVarByteViewShortGetVolatile(java.lang.invoke.VarHandle h, byte[] a, int i) { return (short) h.getVolatile(a, i); }
    public static void jitVarByteViewShortSetOpaque(java.lang.invoke.VarHandle h, byte[] a, int i, short v) { h.setOpaque(a, i, v); }
    public static void jitVarByteViewShortSetRelease(java.lang.invoke.VarHandle h, byte[] a, int i, short v) { h.setRelease(a, i, v); }
    public static void jitVarByteViewShortSetVolatile(java.lang.invoke.VarHandle h, byte[] a, int i, short v) { h.setVolatile(a, i, v); }
    public static boolean jitVarByteViewShortWeakPlain(java.lang.invoke.VarHandle h, byte[] a, int i, short e, short v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarByteViewShortWeakAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, short e, short v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarByteViewShortWeakRelease(java.lang.invoke.VarHandle h, byte[] a, int i, short e, short v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarByteViewShortWeakVolatile(java.lang.invoke.VarHandle h, byte[] a, int i, short e, short v) { return h.weakCompareAndSet(a, i, e, v); }
    public static short jitVarByteViewShortExchangeAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, short e, short v) { return (short) h.compareAndExchangeAcquire(a, i, e, v); }
    public static short jitVarByteViewShortExchangeRelease(java.lang.invoke.VarHandle h, byte[] a, int i, short e, short v) { return (short) h.compareAndExchangeRelease(a, i, e, v); }
    public static short jitVarByteViewShortSwapAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, short v) { return (short) h.getAndSetAcquire(a, i, v); }
    public static short jitVarByteViewShortSwapRelease(java.lang.invoke.VarHandle h, byte[] a, int i, short v) { return (short) h.getAndSetRelease(a, i, v); }
    public static short jitVarByteViewShortAddAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, short v) { return (short) h.getAndAddAcquire(a, i, v); }
    public static short jitVarByteViewShortAddRelease(java.lang.invoke.VarHandle h, byte[] a, int i, short v) { return (short) h.getAndAddRelease(a, i, v); }
    public static short jitVarByteViewShortOr(java.lang.invoke.VarHandle h, byte[] a, int i, short v) { return (short) h.getAndBitwiseOr(a, i, v); }
    public static short jitVarByteViewShortOrAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, short v) { return (short) h.getAndBitwiseOrAcquire(a, i, v); }
    public static short jitVarByteViewShortOrRelease(java.lang.invoke.VarHandle h, byte[] a, int i, short v) { return (short) h.getAndBitwiseOrRelease(a, i, v); }
    public static short jitVarByteViewShortAnd(java.lang.invoke.VarHandle h, byte[] a, int i, short v) { return (short) h.getAndBitwiseAnd(a, i, v); }
    public static short jitVarByteViewShortAndAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, short v) { return (short) h.getAndBitwiseAndAcquire(a, i, v); }
    public static short jitVarByteViewShortAndRelease(java.lang.invoke.VarHandle h, byte[] a, int i, short v) { return (short) h.getAndBitwiseAndRelease(a, i, v); }
    public static short jitVarByteViewShortXorAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, short v) { return (short) h.getAndBitwiseXorAcquire(a, i, v); }
    public static short jitVarByteViewShortXorRelease(java.lang.invoke.VarHandle h, byte[] a, int i, short v) { return (short) h.getAndBitwiseXorRelease(a, i, v); }
    public static char jitVarByteViewCharGetOpaque(java.lang.invoke.VarHandle h, byte[] a, int i) { return (char) h.getOpaque(a, i); }
    public static char jitVarByteViewCharGetAcquire(java.lang.invoke.VarHandle h, byte[] a, int i) { return (char) h.getAcquire(a, i); }
    public static char jitVarByteViewCharGetVolatile(java.lang.invoke.VarHandle h, byte[] a, int i) { return (char) h.getVolatile(a, i); }
    public static void jitVarByteViewCharSetOpaque(java.lang.invoke.VarHandle h, byte[] a, int i, char v) { h.setOpaque(a, i, v); }
    public static void jitVarByteViewCharSetRelease(java.lang.invoke.VarHandle h, byte[] a, int i, char v) { h.setRelease(a, i, v); }
    public static void jitVarByteViewCharSetVolatile(java.lang.invoke.VarHandle h, byte[] a, int i, char v) { h.setVolatile(a, i, v); }
    public static boolean jitVarByteViewCharWeakPlain(java.lang.invoke.VarHandle h, byte[] a, int i, char e, char v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarByteViewCharWeakAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, char e, char v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarByteViewCharWeakRelease(java.lang.invoke.VarHandle h, byte[] a, int i, char e, char v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarByteViewCharWeakVolatile(java.lang.invoke.VarHandle h, byte[] a, int i, char e, char v) { return h.weakCompareAndSet(a, i, e, v); }
    public static char jitVarByteViewCharExchangeAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, char e, char v) { return (char) h.compareAndExchangeAcquire(a, i, e, v); }
    public static char jitVarByteViewCharExchangeRelease(java.lang.invoke.VarHandle h, byte[] a, int i, char e, char v) { return (char) h.compareAndExchangeRelease(a, i, e, v); }
    public static char jitVarByteViewCharSwapAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, char v) { return (char) h.getAndSetAcquire(a, i, v); }
    public static char jitVarByteViewCharSwapRelease(java.lang.invoke.VarHandle h, byte[] a, int i, char v) { return (char) h.getAndSetRelease(a, i, v); }
    public static char jitVarByteViewCharAddAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, char v) { return (char) h.getAndAddAcquire(a, i, v); }
    public static char jitVarByteViewCharAddRelease(java.lang.invoke.VarHandle h, byte[] a, int i, char v) { return (char) h.getAndAddRelease(a, i, v); }
    public static char jitVarByteViewCharOr(java.lang.invoke.VarHandle h, byte[] a, int i, char v) { return (char) h.getAndBitwiseOr(a, i, v); }
    public static char jitVarByteViewCharOrAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, char v) { return (char) h.getAndBitwiseOrAcquire(a, i, v); }
    public static char jitVarByteViewCharOrRelease(java.lang.invoke.VarHandle h, byte[] a, int i, char v) { return (char) h.getAndBitwiseOrRelease(a, i, v); }
    public static char jitVarByteViewCharAnd(java.lang.invoke.VarHandle h, byte[] a, int i, char v) { return (char) h.getAndBitwiseAnd(a, i, v); }
    public static char jitVarByteViewCharAndAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, char v) { return (char) h.getAndBitwiseAndAcquire(a, i, v); }
    public static char jitVarByteViewCharAndRelease(java.lang.invoke.VarHandle h, byte[] a, int i, char v) { return (char) h.getAndBitwiseAndRelease(a, i, v); }
    public static char jitVarByteViewCharXorAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, char v) { return (char) h.getAndBitwiseXorAcquire(a, i, v); }
    public static char jitVarByteViewCharXorRelease(java.lang.invoke.VarHandle h, byte[] a, int i, char v) { return (char) h.getAndBitwiseXorRelease(a, i, v); }
    public static short jitVarBufferViewShortGetOpaque(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i) { return (short) h.getOpaque(a, i); }
    public static short jitVarBufferViewShortGetAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i) { return (short) h.getAcquire(a, i); }
    public static short jitVarBufferViewShortGetVolatile(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i) { return (short) h.getVolatile(a, i); }
    public static void jitVarBufferViewShortSetOpaque(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short v) { h.setOpaque(a, i, v); }
    public static void jitVarBufferViewShortSetRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short v) { h.setRelease(a, i, v); }
    public static void jitVarBufferViewShortSetVolatile(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short v) { h.setVolatile(a, i, v); }
    public static boolean jitVarBufferViewShortWeakPlain(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short e, short v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarBufferViewShortWeakAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short e, short v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarBufferViewShortWeakRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short e, short v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarBufferViewShortWeakVolatile(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short e, short v) { return h.weakCompareAndSet(a, i, e, v); }
    public static short jitVarBufferViewShortExchangeAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short e, short v) { return (short) h.compareAndExchangeAcquire(a, i, e, v); }
    public static short jitVarBufferViewShortExchangeRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short e, short v) { return (short) h.compareAndExchangeRelease(a, i, e, v); }
    public static short jitVarBufferViewShortSwapAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short v) { return (short) h.getAndSetAcquire(a, i, v); }
    public static short jitVarBufferViewShortSwapRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short v) { return (short) h.getAndSetRelease(a, i, v); }
    public static short jitVarBufferViewShortAddAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short v) { return (short) h.getAndAddAcquire(a, i, v); }
    public static short jitVarBufferViewShortAddRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short v) { return (short) h.getAndAddRelease(a, i, v); }
    public static short jitVarBufferViewShortOr(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short v) { return (short) h.getAndBitwiseOr(a, i, v); }
    public static short jitVarBufferViewShortOrAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short v) { return (short) h.getAndBitwiseOrAcquire(a, i, v); }
    public static short jitVarBufferViewShortOrRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short v) { return (short) h.getAndBitwiseOrRelease(a, i, v); }
    public static short jitVarBufferViewShortAnd(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short v) { return (short) h.getAndBitwiseAnd(a, i, v); }
    public static short jitVarBufferViewShortAndAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short v) { return (short) h.getAndBitwiseAndAcquire(a, i, v); }
    public static short jitVarBufferViewShortAndRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short v) { return (short) h.getAndBitwiseAndRelease(a, i, v); }
    public static short jitVarBufferViewShortXorAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short v) { return (short) h.getAndBitwiseXorAcquire(a, i, v); }
    public static short jitVarBufferViewShortXorRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, short v) { return (short) h.getAndBitwiseXorRelease(a, i, v); }
    public static char jitVarBufferViewCharGetOpaque(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i) { return (char) h.getOpaque(a, i); }
    public static char jitVarBufferViewCharGetAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i) { return (char) h.getAcquire(a, i); }
    public static char jitVarBufferViewCharGetVolatile(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i) { return (char) h.getVolatile(a, i); }
    public static void jitVarBufferViewCharSetOpaque(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char v) { h.setOpaque(a, i, v); }
    public static void jitVarBufferViewCharSetRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char v) { h.setRelease(a, i, v); }
    public static void jitVarBufferViewCharSetVolatile(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char v) { h.setVolatile(a, i, v); }
    public static boolean jitVarBufferViewCharWeakPlain(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char e, char v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarBufferViewCharWeakAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char e, char v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarBufferViewCharWeakRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char e, char v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarBufferViewCharWeakVolatile(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char e, char v) { return h.weakCompareAndSet(a, i, e, v); }
    public static char jitVarBufferViewCharExchangeAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char e, char v) { return (char) h.compareAndExchangeAcquire(a, i, e, v); }
    public static char jitVarBufferViewCharExchangeRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char e, char v) { return (char) h.compareAndExchangeRelease(a, i, e, v); }
    public static char jitVarBufferViewCharSwapAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char v) { return (char) h.getAndSetAcquire(a, i, v); }
    public static char jitVarBufferViewCharSwapRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char v) { return (char) h.getAndSetRelease(a, i, v); }
    public static char jitVarBufferViewCharAddAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char v) { return (char) h.getAndAddAcquire(a, i, v); }
    public static char jitVarBufferViewCharAddRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char v) { return (char) h.getAndAddRelease(a, i, v); }
    public static char jitVarBufferViewCharOr(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char v) { return (char) h.getAndBitwiseOr(a, i, v); }
    public static char jitVarBufferViewCharOrAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char v) { return (char) h.getAndBitwiseOrAcquire(a, i, v); }
    public static char jitVarBufferViewCharOrRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char v) { return (char) h.getAndBitwiseOrRelease(a, i, v); }
    public static char jitVarBufferViewCharAnd(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char v) { return (char) h.getAndBitwiseAnd(a, i, v); }
    public static char jitVarBufferViewCharAndAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char v) { return (char) h.getAndBitwiseAndAcquire(a, i, v); }
    public static char jitVarBufferViewCharAndRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char v) { return (char) h.getAndBitwiseAndRelease(a, i, v); }
    public static char jitVarBufferViewCharXorAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char v) { return (char) h.getAndBitwiseXorAcquire(a, i, v); }
    public static char jitVarBufferViewCharXorRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, char v) { return (char) h.getAndBitwiseXorRelease(a, i, v); }
    public static float jitVarByteViewFloatGetOpaque(java.lang.invoke.VarHandle h, byte[] a, int i) { return (float) h.getOpaque(a, i); }
    public static float jitVarByteViewFloatGetAcquire(java.lang.invoke.VarHandle h, byte[] a, int i) { return (float) h.getAcquire(a, i); }
    public static float jitVarByteViewFloatGetVolatile(java.lang.invoke.VarHandle h, byte[] a, int i) { return (float) h.getVolatile(a, i); }
    public static void jitVarByteViewFloatSetOpaque(java.lang.invoke.VarHandle h, byte[] a, int i, float v) { h.setOpaque(a, i, v); }
    public static void jitVarByteViewFloatSetRelease(java.lang.invoke.VarHandle h, byte[] a, int i, float v) { h.setRelease(a, i, v); }
    public static void jitVarByteViewFloatSetVolatile(java.lang.invoke.VarHandle h, byte[] a, int i, float v) { h.setVolatile(a, i, v); }
    public static boolean jitVarByteViewFloatWeakPlain(java.lang.invoke.VarHandle h, byte[] a, int i, float e, float v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarByteViewFloatWeakAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, float e, float v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarByteViewFloatWeakRelease(java.lang.invoke.VarHandle h, byte[] a, int i, float e, float v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarByteViewFloatWeakVolatile(java.lang.invoke.VarHandle h, byte[] a, int i, float e, float v) { return h.weakCompareAndSet(a, i, e, v); }
    public static float jitVarByteViewFloatExchangeAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, float e, float v) { return (float) h.compareAndExchangeAcquire(a, i, e, v); }
    public static float jitVarByteViewFloatExchangeRelease(java.lang.invoke.VarHandle h, byte[] a, int i, float e, float v) { return (float) h.compareAndExchangeRelease(a, i, e, v); }
    public static float jitVarByteViewFloatSwapAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, float v) { return (float) h.getAndSetAcquire(a, i, v); }
    public static float jitVarByteViewFloatSwapRelease(java.lang.invoke.VarHandle h, byte[] a, int i, float v) { return (float) h.getAndSetRelease(a, i, v); }
    public static float jitVarByteViewFloatAddAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, float v) { return (float) h.getAndAddAcquire(a, i, v); }
    public static float jitVarByteViewFloatAddRelease(java.lang.invoke.VarHandle h, byte[] a, int i, float v) { return (float) h.getAndAddRelease(a, i, v); }
    public static float jitVarByteViewFloatOr(java.lang.invoke.VarHandle h, byte[] a, int i, float v) { return (float) h.getAndBitwiseOr(a, i, v); }
    public static float jitVarByteViewFloatOrAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, float v) { return (float) h.getAndBitwiseOrAcquire(a, i, v); }
    public static float jitVarByteViewFloatOrRelease(java.lang.invoke.VarHandle h, byte[] a, int i, float v) { return (float) h.getAndBitwiseOrRelease(a, i, v); }
    public static float jitVarByteViewFloatAnd(java.lang.invoke.VarHandle h, byte[] a, int i, float v) { return (float) h.getAndBitwiseAnd(a, i, v); }
    public static float jitVarByteViewFloatAndAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, float v) { return (float) h.getAndBitwiseAndAcquire(a, i, v); }
    public static float jitVarByteViewFloatAndRelease(java.lang.invoke.VarHandle h, byte[] a, int i, float v) { return (float) h.getAndBitwiseAndRelease(a, i, v); }
    public static float jitVarByteViewFloatXorAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, float v) { return (float) h.getAndBitwiseXorAcquire(a, i, v); }
    public static float jitVarByteViewFloatXorRelease(java.lang.invoke.VarHandle h, byte[] a, int i, float v) { return (float) h.getAndBitwiseXorRelease(a, i, v); }
    public static double jitVarByteViewDoubleGetOpaque(java.lang.invoke.VarHandle h, byte[] a, int i) { return (double) h.getOpaque(a, i); }
    public static double jitVarByteViewDoubleGetAcquire(java.lang.invoke.VarHandle h, byte[] a, int i) { return (double) h.getAcquire(a, i); }
    public static double jitVarByteViewDoubleGetVolatile(java.lang.invoke.VarHandle h, byte[] a, int i) { return (double) h.getVolatile(a, i); }
    public static void jitVarByteViewDoubleSetOpaque(java.lang.invoke.VarHandle h, byte[] a, int i, double v) { h.setOpaque(a, i, v); }
    public static void jitVarByteViewDoubleSetRelease(java.lang.invoke.VarHandle h, byte[] a, int i, double v) { h.setRelease(a, i, v); }
    public static void jitVarByteViewDoubleSetVolatile(java.lang.invoke.VarHandle h, byte[] a, int i, double v) { h.setVolatile(a, i, v); }
    public static boolean jitVarByteViewDoubleWeakPlain(java.lang.invoke.VarHandle h, byte[] a, int i, double e, double v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarByteViewDoubleWeakAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, double e, double v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarByteViewDoubleWeakRelease(java.lang.invoke.VarHandle h, byte[] a, int i, double e, double v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarByteViewDoubleWeakVolatile(java.lang.invoke.VarHandle h, byte[] a, int i, double e, double v) { return h.weakCompareAndSet(a, i, e, v); }
    public static double jitVarByteViewDoubleExchangeAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, double e, double v) { return (double) h.compareAndExchangeAcquire(a, i, e, v); }
    public static double jitVarByteViewDoubleExchangeRelease(java.lang.invoke.VarHandle h, byte[] a, int i, double e, double v) { return (double) h.compareAndExchangeRelease(a, i, e, v); }
    public static double jitVarByteViewDoubleSwapAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, double v) { return (double) h.getAndSetAcquire(a, i, v); }
    public static double jitVarByteViewDoubleSwapRelease(java.lang.invoke.VarHandle h, byte[] a, int i, double v) { return (double) h.getAndSetRelease(a, i, v); }
    public static double jitVarByteViewDoubleAddAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, double v) { return (double) h.getAndAddAcquire(a, i, v); }
    public static double jitVarByteViewDoubleAddRelease(java.lang.invoke.VarHandle h, byte[] a, int i, double v) { return (double) h.getAndAddRelease(a, i, v); }
    public static double jitVarByteViewDoubleOr(java.lang.invoke.VarHandle h, byte[] a, int i, double v) { return (double) h.getAndBitwiseOr(a, i, v); }
    public static double jitVarByteViewDoubleOrAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, double v) { return (double) h.getAndBitwiseOrAcquire(a, i, v); }
    public static double jitVarByteViewDoubleOrRelease(java.lang.invoke.VarHandle h, byte[] a, int i, double v) { return (double) h.getAndBitwiseOrRelease(a, i, v); }
    public static double jitVarByteViewDoubleAnd(java.lang.invoke.VarHandle h, byte[] a, int i, double v) { return (double) h.getAndBitwiseAnd(a, i, v); }
    public static double jitVarByteViewDoubleAndAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, double v) { return (double) h.getAndBitwiseAndAcquire(a, i, v); }
    public static double jitVarByteViewDoubleAndRelease(java.lang.invoke.VarHandle h, byte[] a, int i, double v) { return (double) h.getAndBitwiseAndRelease(a, i, v); }
    public static double jitVarByteViewDoubleXorAcquire(java.lang.invoke.VarHandle h, byte[] a, int i, double v) { return (double) h.getAndBitwiseXorAcquire(a, i, v); }
    public static double jitVarByteViewDoubleXorRelease(java.lang.invoke.VarHandle h, byte[] a, int i, double v) { return (double) h.getAndBitwiseXorRelease(a, i, v); }
    public static float jitVarBufferViewFloatGetOpaque(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i) { return (float) h.getOpaque(a, i); }
    public static float jitVarBufferViewFloatGetAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i) { return (float) h.getAcquire(a, i); }
    public static float jitVarBufferViewFloatGetVolatile(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i) { return (float) h.getVolatile(a, i); }
    public static void jitVarBufferViewFloatSetOpaque(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float v) { h.setOpaque(a, i, v); }
    public static void jitVarBufferViewFloatSetRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float v) { h.setRelease(a, i, v); }
    public static void jitVarBufferViewFloatSetVolatile(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float v) { h.setVolatile(a, i, v); }
    public static boolean jitVarBufferViewFloatWeakPlain(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float e, float v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarBufferViewFloatWeakAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float e, float v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarBufferViewFloatWeakRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float e, float v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarBufferViewFloatWeakVolatile(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float e, float v) { return h.weakCompareAndSet(a, i, e, v); }
    public static float jitVarBufferViewFloatExchangeAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float e, float v) { return (float) h.compareAndExchangeAcquire(a, i, e, v); }
    public static float jitVarBufferViewFloatExchangeRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float e, float v) { return (float) h.compareAndExchangeRelease(a, i, e, v); }
    public static float jitVarBufferViewFloatSwapAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float v) { return (float) h.getAndSetAcquire(a, i, v); }
    public static float jitVarBufferViewFloatSwapRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float v) { return (float) h.getAndSetRelease(a, i, v); }
    public static float jitVarBufferViewFloatAddAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float v) { return (float) h.getAndAddAcquire(a, i, v); }
    public static float jitVarBufferViewFloatAddRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float v) { return (float) h.getAndAddRelease(a, i, v); }
    public static float jitVarBufferViewFloatOr(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float v) { return (float) h.getAndBitwiseOr(a, i, v); }
    public static float jitVarBufferViewFloatOrAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float v) { return (float) h.getAndBitwiseOrAcquire(a, i, v); }
    public static float jitVarBufferViewFloatOrRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float v) { return (float) h.getAndBitwiseOrRelease(a, i, v); }
    public static float jitVarBufferViewFloatAnd(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float v) { return (float) h.getAndBitwiseAnd(a, i, v); }
    public static float jitVarBufferViewFloatAndAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float v) { return (float) h.getAndBitwiseAndAcquire(a, i, v); }
    public static float jitVarBufferViewFloatAndRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float v) { return (float) h.getAndBitwiseAndRelease(a, i, v); }
    public static float jitVarBufferViewFloatXorAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float v) { return (float) h.getAndBitwiseXorAcquire(a, i, v); }
    public static float jitVarBufferViewFloatXorRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, float v) { return (float) h.getAndBitwiseXorRelease(a, i, v); }
    public static double jitVarBufferViewDoubleGetOpaque(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i) { return (double) h.getOpaque(a, i); }
    public static double jitVarBufferViewDoubleGetAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i) { return (double) h.getAcquire(a, i); }
    public static double jitVarBufferViewDoubleGetVolatile(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i) { return (double) h.getVolatile(a, i); }
    public static void jitVarBufferViewDoubleSetOpaque(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double v) { h.setOpaque(a, i, v); }
    public static void jitVarBufferViewDoubleSetRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double v) { h.setRelease(a, i, v); }
    public static void jitVarBufferViewDoubleSetVolatile(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double v) { h.setVolatile(a, i, v); }
    public static boolean jitVarBufferViewDoubleWeakPlain(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double e, double v) { return h.weakCompareAndSetPlain(a, i, e, v); }
    public static boolean jitVarBufferViewDoubleWeakAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double e, double v) { return h.weakCompareAndSetAcquire(a, i, e, v); }
    public static boolean jitVarBufferViewDoubleWeakRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double e, double v) { return h.weakCompareAndSetRelease(a, i, e, v); }
    public static boolean jitVarBufferViewDoubleWeakVolatile(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double e, double v) { return h.weakCompareAndSet(a, i, e, v); }
    public static double jitVarBufferViewDoubleExchangeAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double e, double v) { return (double) h.compareAndExchangeAcquire(a, i, e, v); }
    public static double jitVarBufferViewDoubleExchangeRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double e, double v) { return (double) h.compareAndExchangeRelease(a, i, e, v); }
    public static double jitVarBufferViewDoubleSwapAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double v) { return (double) h.getAndSetAcquire(a, i, v); }
    public static double jitVarBufferViewDoubleSwapRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double v) { return (double) h.getAndSetRelease(a, i, v); }
    public static double jitVarBufferViewDoubleAddAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double v) { return (double) h.getAndAddAcquire(a, i, v); }
    public static double jitVarBufferViewDoubleAddRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double v) { return (double) h.getAndAddRelease(a, i, v); }
    public static double jitVarBufferViewDoubleOr(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double v) { return (double) h.getAndBitwiseOr(a, i, v); }
    public static double jitVarBufferViewDoubleOrAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double v) { return (double) h.getAndBitwiseOrAcquire(a, i, v); }
    public static double jitVarBufferViewDoubleOrRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double v) { return (double) h.getAndBitwiseOrRelease(a, i, v); }
    public static double jitVarBufferViewDoubleAnd(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double v) { return (double) h.getAndBitwiseAnd(a, i, v); }
    public static double jitVarBufferViewDoubleAndAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double v) { return (double) h.getAndBitwiseAndAcquire(a, i, v); }
    public static double jitVarBufferViewDoubleAndRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double v) { return (double) h.getAndBitwiseAndRelease(a, i, v); }
    public static double jitVarBufferViewDoubleXorAcquire(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double v) { return (double) h.getAndBitwiseXorAcquire(a, i, v); }
    public static double jitVarBufferViewDoubleXorRelease(java.lang.invoke.VarHandle h, java.nio.ByteBuffer a, int i, double v) { return (double) h.getAndBitwiseXorRelease(a, i, v); }
    public static Object jitHandleStaticReference;
    public static java.lang.invoke.MethodHandle jitPolymorphicAccessorHandle(int kind) throws Exception {
        java.lang.invoke.MethodHandles.Lookup lookup = java.lang.invoke.MethodHandles.lookup();
        switch (kind) {
            case 0: return lookup.findGetter(Hello.class, "jitHandleReference", Object.class);
            case 1: return lookup.findSetter(Hello.class, "jitHandleReference", Object.class);
            case 2: return lookup.findStaticGetter(Hello.class, "jitHandleStaticReference", Object.class);
            case 3: return lookup.findStaticSetter(Hello.class, "jitHandleStaticReference", Object.class);
            default: throw new IllegalArgumentException();
        }
    }
    public static Object jitPolymorphicGet(java.lang.invoke.MethodHandle handle, Hello receiver) throws Throwable {
        return (Object) handle.invokeExact(receiver);
    }
    public static void jitPolymorphicPut(java.lang.invoke.MethodHandle handle, Hello receiver, Object value) throws Throwable {
        handle.invokeExact(receiver, value);
    }
    public static Object jitPolymorphicStaticGet(java.lang.invoke.MethodHandle handle) throws Throwable {
        return (Object) handle.invokeExact();
    }
    public static void jitPolymorphicStaticPut(java.lang.invoke.MethodHandle handle, Object value) throws Throwable {
        handle.invokeExact(value);
    }
    public static java.lang.invoke.MethodHandle jitPolymorphicDispatchHandle(boolean interfaced) throws Exception {
        return java.lang.invoke.MethodHandles.lookup().findVirtual(
                interfaced ? JitCallable.class : JitVirtualBase.class, "value",
                java.lang.invoke.MethodType.methodType(int.class));
    }
    public static int jitPolymorphicVirtual(java.lang.invoke.MethodHandle handle, JitVirtualBase value) throws Throwable {
        return (int) handle.invokeExact(value);
    }
    public static int jitPolymorphicInterface(java.lang.invoke.MethodHandle handle, JitCallable value) throws Throwable {
        return (int) handle.invokeExact(value);
    }
    public static java.lang.invoke.MethodHandle jitPolymorphicRangeHandle() throws Exception {
        return java.lang.invoke.MethodHandles.lookup().findStatic(Hello.class, "jitRangeTarget",
                java.lang.invoke.MethodType.methodType(long.class, int.class, int.class, int.class,
                        int.class, int.class, int.class, int.class, long.class, double.class, Object.class));
    }
    public static long jitPolymorphicRange(java.lang.invoke.MethodHandle handle,
            int a, int b, int c, int d, int e, int f, int g, long wide, double real, Object value) throws Throwable {
        return (long) handle.invokeExact(a, b, c, d, e, f, g, wide, real, value);
    }
    public static java.lang.invoke.MethodHandle jitPolymorphicDoubleHandle() throws Exception {
        return java.lang.invoke.MethodHandles.lookup().findStatic(Hello.class, "jitPolymorphicDoubleTarget",
                java.lang.invoke.MethodType.methodType(double.class, double.class, double.class, double.class, double.class, double.class, double.class, double.class, double.class, double.class, Object.class));
    }
    public static double jitPolymorphicDoubleTarget(double a, double b, double c, double d, double e, double f, double g, double h, double i, Object value) {
        System.gc();
        return a == 1 && b == 2 && c == 3 && d == 4 && e == 5 && f == 6 && g == 7 && h == 8 && value != null ? i : (double) -1234.5;
    }
    public static double jitPolymorphicDouble(java.lang.invoke.MethodHandle handle,
            double a, double b, double c, double d, double e, double f, double g, double h, double i, Object value) throws Throwable {
        return (double) handle.invokeExact(a, b, c, d, e, f, g, h, i, value);
    }
    public static java.lang.invoke.MethodHandle jitPolymorphicFloatHandle() throws Exception {
        return java.lang.invoke.MethodHandles.lookup().findStatic(Hello.class, "jitPolymorphicFloatTarget",
                java.lang.invoke.MethodType.methodType(float.class, float.class, float.class, float.class, float.class, float.class, float.class, float.class, float.class, float.class, Object.class));
    }
    public static float jitPolymorphicFloatTarget(float a, float b, float c, float d, float e, float f, float g, float h, float i, Object value) {
        System.gc();
        return a == 1 && b == 2 && c == 3 && d == 4 && e == 5 && f == 6 && g == 7 && h == 8 && value != null ? i : (float) -1234.5;
    }
    public static float jitPolymorphicFloat(java.lang.invoke.MethodHandle handle,
            float a, float b, float c, float d, float e, float f, float g, float h, float i, Object value) throws Throwable {
        return (float) handle.invokeExact(a, b, c, d, e, f, g, h, i, value);
    }
    public static byte[] jitLiteralBytes() { return new byte[] {-128, -1, 0, 1, 127}; }
    public static short[] jitLiteralShorts() { return new short[] {-32768, -1, 0, 1, 32767}; }
    public static int[] jitLiteralInts() { return new int[] {Integer.MIN_VALUE, -1, 0, 1, Integer.MAX_VALUE}; }
    public static long[] jitLiteralLongs() { return new long[] {Long.MIN_VALUE, -1, 0, 1, Long.MAX_VALUE}; }
    public static double[] jitLiteralDoubles() { return new double[] {-1234.5, -0.0, 0.0, 1.0, Double.MAX_VALUE}; }
    public static float[] jitLiteralFloats() { return new float[] {-1234.5f, -0.0f, 0.0f, 1.0f, Float.MAX_VALUE}; }
    public static Object[] jitLiteralReferences(Object a, Object b) { return new Object[] {a, null, b}; }
    public static Object[] jitLiteralReferenceRange(Object a, Object b, Object c, Object d, Object e, Object f, Object g) {
        return new Object[] {a, b, c, d, e, f, g};
    }
    // Intentionally unrolled: exercises code generation beyond the former 256-unit cap.
    public static int jitLargeArithmetic(int value) {
        value = (value * 33) ^ 7;
        value = (value * 33) ^ 12352;
        value = (value * 33) ^ 24697;
        value = (value * 33) ^ 37042;
        value = (value * 33) ^ 49387;
        value = (value * 33) ^ 61732;
        value = (value * 33) ^ 74077;
        value = (value * 33) ^ 86422;
        value = (value * 33) ^ 98767;
        value = (value * 33) ^ 111112;
        value = (value * 33) ^ 123457;
        value = (value * 33) ^ 135802;
        value = (value * 33) ^ 148147;
        value = (value * 33) ^ 160492;
        value = (value * 33) ^ 172837;
        value = (value * 33) ^ 185182;
        value = (value * 33) ^ 197527;
        value = (value * 33) ^ 209872;
        value = (value * 33) ^ 222217;
        value = (value * 33) ^ 234562;
        value = (value * 33) ^ 246907;
        value = (value * 33) ^ 259252;
        value = (value * 33) ^ 271597;
        value = (value * 33) ^ 283942;
        value = (value * 33) ^ 296287;
        value = (value * 33) ^ 308632;
        value = (value * 33) ^ 320977;
        value = (value * 33) ^ 333322;
        value = (value * 33) ^ 345667;
        value = (value * 33) ^ 358012;
        value = (value * 33) ^ 370357;
        value = (value * 33) ^ 382702;
        value = (value * 33) ^ 395047;
        value = (value * 33) ^ 407392;
        value = (value * 33) ^ 419737;
        value = (value * 33) ^ 432082;
        value = (value * 33) ^ 444427;
        value = (value * 33) ^ 456772;
        value = (value * 33) ^ 469117;
        value = (value * 33) ^ 481462;
        value = (value * 33) ^ 493807;
        value = (value * 33) ^ 506152;
        value = (value * 33) ^ 518497;
        value = (value * 33) ^ 530842;
        value = (value * 33) ^ 543187;
        value = (value * 33) ^ 555532;
        value = (value * 33) ^ 567877;
        value = (value * 33) ^ 580222;
        value = (value * 33) ^ 592567;
        value = (value * 33) ^ 604912;
        value = (value * 33) ^ 617257;
        value = (value * 33) ^ 629602;
        value = (value * 33) ^ 641947;
        value = (value * 33) ^ 654292;
        value = (value * 33) ^ 666637;
        value = (value * 33) ^ 678982;
        value = (value * 33) ^ 691327;
        value = (value * 33) ^ 703672;
        value = (value * 33) ^ 716017;
        value = (value * 33) ^ 728362;
        value = (value * 33) ^ 740707;
        value = (value * 33) ^ 753052;
        value = (value * 33) ^ 765397;
        value = (value * 33) ^ 777742;
        value = (value * 33) ^ 790087;
        value = (value * 33) ^ 802432;
        value = (value * 33) ^ 814777;
        value = (value * 33) ^ 827122;
        value = (value * 33) ^ 839467;
        value = (value * 33) ^ 851812;
        value = (value * 33) ^ 864157;
        value = (value * 33) ^ 876502;
        value = (value * 33) ^ 888847;
        value = (value * 33) ^ 901192;
        value = (value * 33) ^ 913537;
        value = (value * 33) ^ 925882;
        value = (value * 33) ^ 938227;
        value = (value * 33) ^ 950572;
        value = (value * 33) ^ 962917;
        value = (value * 33) ^ 975262;
        value = (value * 33) ^ 987607;
        value = (value * 33) ^ 999952;
        value = (value * 33) ^ 1012297;
        value = (value * 33) ^ 1024642;
        value = (value * 33) ^ 1036987;
        value = (value * 33) ^ 1049332;
        value = (value * 33) ^ 1061677;
        value = (value * 33) ^ 1074022;
        value = (value * 33) ^ 1086367;
        value = (value * 33) ^ 1098712;
        value = (value * 33) ^ 1111057;
        value = (value * 33) ^ 1123402;
        value = (value * 33) ^ 1135747;
        value = (value * 33) ^ 1148092;
        value = (value * 33) ^ 1160437;
        value = (value * 33) ^ 1172782;
        return value;
    }
    public static Class<?> jitMissingClass() { return JitMissingType.class; }
    public static int jitMissingStaticCall() { return JitMissingType.call(); }
    public static Object jitMissingReferenceCall(Object value) { return JitMissingType.reference(value); }
    public static int jitMissingStaticInt() { return JitMissingType.number; }
    public static Object jitMissingStaticReference() { return JitMissingType.object; }
    public static void jitMissingStaticIntSet(int value) { JitMissingType.number = value; }
    public static void jitMissingStaticReferenceSet(Object value) { JitMissingType.object = value; }
    public static boolean jitMissingInstanceOf(Object value) { return value instanceof JitMissingType; }
    public static Object jitMissingCast(Object value) { return (JitMissingType) value; }
    public static native Object nativeReferenceIdentity(Object value);
    public native Object nativeVirtualReceiver();
    private native Object nativeDirectReceiver();
    public static Object jitNativeVirtual(Hello receiver) { return receiver.nativeVirtualReceiver(); }
    public static Object jitNativeDirect(Hello receiver) { return receiver.nativeDirectReceiver(); }
    public static int jitNativePolymorphic(JitVirtualBase receiver) { return receiver.nativeToken(); }
    public static int jitExternalIntGet(JitVirtualBase receiver) { return receiver.number; }
    public static void jitExternalIntSet(JitVirtualBase receiver, int value) { receiver.number = value; }
    public static Object jitExternalRefGet(JitVirtualBase receiver) { return receiver.object; }
    public static void jitExternalRefSet(JitVirtualBase receiver, Object value) { receiver.object = value; }
    public static int jitNativeInterface(JitCallable receiver) { return receiver.nativeToken(); }
    private static native long nativePackedIntegerStack(
            int a0, int a1, int a2, int a3, int a4, int a5, int spilled,
            long wide, int tail0, int tail1);
    private static native long nativePackedFloatingStack(
            float a0, float a1, float a2, float a3, float a4, float a5,
            float a6, float a7, float spilled, double wide);
    private static native long nativePackedReferenceStack(
            int a0, int a1, int a2, int a3, int a4, int a5, int spilled,
            Object reference, int tail);
    private static native long nativePackedNarrowStack(
            int a0, int a1, int a2, int a3, int a4, int a5,
            boolean boolValue, byte byteValue, char charValue, short shortValue,
            int intValue, long longValue);

    public static void main(String[] args) {
        System.out.println(args[0]);
    }

    public static int answer() {
        return 42;
    }

    public static int jitArithmetic(int x, int y) {
        return (x * 31 + y) ^ (x >>> (y & 31));
    }

    public static Object jitIdentity(Object value) {
        return value;
    }
    public static Object jitExitIdentity(Object value) {
        return value;
    }
    public static Object jitRootString() { return "darwin-jit-root-slot-64"; }
    public static Object jitRootClass() { return Hello.class; }
    public static Object jitRootOtherClass() { return ProbeActivity.class; }
    public static boolean[] jitNewArrayZ(int size) { return new boolean[size]; }
    public static byte[] jitNewArrayB(int size) { return new byte[size]; }
    public static char[] jitNewArrayC(int size) { return new char[size]; }
    public static short[] jitNewArrayS(int size) { return new short[size]; }
    public static int[] jitNewArrayI(int size) { return new int[size]; }
    public static long[] jitNewArrayJ(int size) { return new long[size]; }
    public static float[] jitNewArrayF(int size) { return new float[size]; }
    public static double[] jitNewArrayD(int size) { return new double[size]; }
    public static Object[] jitNewArrayL(int size) { return new Object[size]; }
    public static Hello[] jitNewArrayCustom(int size) { return new Hello[size]; }
    public static int[][] jitNewArrayNested(int size) { return new int[size][]; }
    public static int jitArrayLengthZ(boolean[] array) { return array.length; }
    public static boolean jitArrayGetZ(boolean[] array, int index) { return array[index]; }
    public static void jitArraySetZ(boolean[] array, int index, boolean value) { array[index] = value; }
    public static int jitArrayLengthB(byte[] array) { return array.length; }
    public static byte jitArrayGetB(byte[] array, int index) { return array[index]; }
    public static void jitArraySetB(byte[] array, int index, byte value) { array[index] = value; }
    public static int jitArrayLengthC(char[] array) { return array.length; }
    public static char jitArrayGetC(char[] array, int index) { return array[index]; }
    public static void jitArraySetC(char[] array, int index, char value) { array[index] = value; }
    public static int jitArrayLengthS(short[] array) { return array.length; }
    public static short jitArrayGetS(short[] array, int index) { return array[index]; }
    public static void jitArraySetS(short[] array, int index, short value) { array[index] = value; }
    public static int jitArrayLengthI(int[] array) { return array.length; }
    public static int jitArrayGetI(int[] array, int index) { return array[index]; }
    public static void jitArraySetI(int[] array, int index, int value) { array[index] = value; }
    public static int jitArrayLengthJ(long[] array) { return array.length; }
    public static long jitArrayGetJ(long[] array, int index) { return array[index]; }
    public static void jitArraySetJ(long[] array, int index, long value) { array[index] = value; }
    public static int jitArrayLengthF(float[] array) { return array.length; }
    public static float jitArrayGetF(float[] array, int index) { return array[index]; }
    public static void jitArraySetF(float[] array, int index, float value) { array[index] = value; }
    public static int jitArrayLengthD(double[] array) { return array.length; }
    public static double jitArrayGetD(double[] array, int index) { return array[index]; }
    public static void jitArraySetD(double[] array, int index, double value) { array[index] = value; }
    public static int jitArrayLengthL(Object[] array) { return array.length; }
    public static Object jitArrayGetL(Object[] array, int index) { return array[index]; }
    public static void jitArraySetL(Object[] array, int index, Object value) { array[index] = value; }
    public boolean jitFieldIZ;
    public static boolean jitGetIZ(Hello receiver) { return receiver.jitFieldIZ; }
    public static void jitSetIZ(Hello receiver, boolean value) { receiver.jitFieldIZ = value; }
    public byte jitFieldIB;
    public static byte jitGetIB(Hello receiver) { return receiver.jitFieldIB; }
    public static void jitSetIB(Hello receiver, byte value) { receiver.jitFieldIB = value; }
    public char jitFieldIC;
    public static char jitGetIC(Hello receiver) { return receiver.jitFieldIC; }
    public static void jitSetIC(Hello receiver, char value) { receiver.jitFieldIC = value; }
    public short jitFieldIS;
    public static short jitGetIS(Hello receiver) { return receiver.jitFieldIS; }
    public static void jitSetIS(Hello receiver, short value) { receiver.jitFieldIS = value; }
    public int jitFieldII;
    public static int jitGetII(Hello receiver) { return receiver.jitFieldII; }
    public static void jitSetII(Hello receiver, int value) { receiver.jitFieldII = value; }
    public long jitFieldIJ;
    public static long jitGetIJ(Hello receiver) { return receiver.jitFieldIJ; }
    public static void jitSetIJ(Hello receiver, long value) { receiver.jitFieldIJ = value; }
    public float jitFieldIF;
    public static float jitGetIF(Hello receiver) { return receiver.jitFieldIF; }
    public static void jitSetIF(Hello receiver, float value) { receiver.jitFieldIF = value; }
    public double jitFieldID;
    public static double jitGetID(Hello receiver) { return receiver.jitFieldID; }
    public static void jitSetID(Hello receiver, double value) { receiver.jitFieldID = value; }
    public Object jitFieldIL;
    public static Object jitGetIL(Hello receiver) { return receiver.jitFieldIL; }
    public static void jitSetIL(Hello receiver, Object value) { receiver.jitFieldIL = value; }
    public volatile boolean jitFieldVZ;
    public static boolean jitGetVZ(Hello receiver) { return receiver.jitFieldVZ; }
    public static void jitSetVZ(Hello receiver, boolean value) { receiver.jitFieldVZ = value; }
    public volatile byte jitFieldVB;
    public static byte jitGetVB(Hello receiver) { return receiver.jitFieldVB; }
    public static void jitSetVB(Hello receiver, byte value) { receiver.jitFieldVB = value; }
    public volatile char jitFieldVC;
    public static char jitGetVC(Hello receiver) { return receiver.jitFieldVC; }
    public static void jitSetVC(Hello receiver, char value) { receiver.jitFieldVC = value; }
    public volatile short jitFieldVS;
    public static short jitGetVS(Hello receiver) { return receiver.jitFieldVS; }
    public static void jitSetVS(Hello receiver, short value) { receiver.jitFieldVS = value; }
    public volatile int jitFieldVI;
    public static int jitGetVI(Hello receiver) { return receiver.jitFieldVI; }
    public static void jitSetVI(Hello receiver, int value) { receiver.jitFieldVI = value; }
    public volatile long jitFieldVJ;
    public static long jitGetVJ(Hello receiver) { return receiver.jitFieldVJ; }
    public static void jitSetVJ(Hello receiver, long value) { receiver.jitFieldVJ = value; }
    public volatile float jitFieldVF;
    public static float jitGetVF(Hello receiver) { return receiver.jitFieldVF; }
    public static void jitSetVF(Hello receiver, float value) { receiver.jitFieldVF = value; }
    public volatile double jitFieldVD;
    public static double jitGetVD(Hello receiver) { return receiver.jitFieldVD; }
    public static void jitSetVD(Hello receiver, double value) { receiver.jitFieldVD = value; }
    public volatile Object jitFieldVL;
    public static Object jitGetVL(Hello receiver) { return receiver.jitFieldVL; }
    public static void jitSetVL(Hello receiver, Object value) { receiver.jitFieldVL = value; }
    public static boolean jitFieldSZ;
    public static boolean jitGetSZ() { return jitFieldSZ; }
    public static void jitSetSZ(boolean value) { jitFieldSZ = value; }
    public static byte jitFieldSB;
    public static byte jitGetSB() { return jitFieldSB; }
    public static void jitSetSB(byte value) { jitFieldSB = value; }
    public static char jitFieldSC;
    public static char jitGetSC() { return jitFieldSC; }
    public static void jitSetSC(char value) { jitFieldSC = value; }
    public static short jitFieldSS;
    public static short jitGetSS() { return jitFieldSS; }
    public static void jitSetSS(short value) { jitFieldSS = value; }
    public static int jitFieldSI;
    public static int jitGetSI() { return jitFieldSI; }
    public static void jitSetSI(int value) { jitFieldSI = value; }
    public static long jitFieldSJ;
    public static long jitGetSJ() { return jitFieldSJ; }
    public static void jitSetSJ(long value) { jitFieldSJ = value; }
    public static float jitFieldSF;
    public static float jitGetSF() { return jitFieldSF; }
    public static void jitSetSF(float value) { jitFieldSF = value; }
    public static double jitFieldSD;
    public static double jitGetSD() { return jitFieldSD; }
    public static void jitSetSD(double value) { jitFieldSD = value; }
    public static Object jitFieldSL;
    public static Object jitGetSL() { return jitFieldSL; }
    public static void jitSetSL(Object value) { jitFieldSL = value; }
    public static volatile boolean jitFieldTZ;
    public static boolean jitGetTZ() { return jitFieldTZ; }
    public static void jitSetTZ(boolean value) { jitFieldTZ = value; }
    public static volatile byte jitFieldTB;
    public static byte jitGetTB() { return jitFieldTB; }
    public static void jitSetTB(byte value) { jitFieldTB = value; }
    public static volatile char jitFieldTC;
    public static char jitGetTC() { return jitFieldTC; }
    public static void jitSetTC(char value) { jitFieldTC = value; }
    public static volatile short jitFieldTS;
    public static short jitGetTS() { return jitFieldTS; }
    public static void jitSetTS(short value) { jitFieldTS = value; }
    public static volatile int jitFieldTI;
    public static int jitGetTI() { return jitFieldTI; }
    public static void jitSetTI(int value) { jitFieldTI = value; }
    public static volatile long jitFieldTJ;
    public static long jitGetTJ() { return jitFieldTJ; }
    public static void jitSetTJ(long value) { jitFieldTJ = value; }
    public static volatile float jitFieldTF;
    public static float jitGetTF() { return jitFieldTF; }
    public static void jitSetTF(float value) { jitFieldTF = value; }
    public static volatile double jitFieldTD;
    public static double jitGetTD() { return jitFieldTD; }
    public static void jitSetTD(double value) { jitFieldTD = value; }
    public static volatile Object jitFieldTL;
    public static Object jitGetTL() { return jitFieldTL; }
    public static void jitSetTL(Object value) { jitFieldTL = value; }

    public int jitIntField;
    public Object jitReferenceField;

    // build-dex-probe expands this marker with a large set of padding fields
    // before the target fields. Keeping the expansion build-time generated
    // keeps this fixture reviewable while exercising real large offsets.
    // AIM_LARGE_FIELD_FIXTURE

    public static int jitReadInt(Hello value) { return value.jitIntField; }
    public static Object jitReadReference(Hello value) { return value.jitReferenceField; }
    public static int jitReadLargeInt(Hello value) { return value.jitLargeIntField; }
    public static Object jitReadLargeReference(Hello value) { return value.jitLargeReferenceField; }
    public static int jitReadLargeVolatileInt(Hello value) { return value.jitLargeVolatileIntField; }
    public static int jitReadLargeIntTwice(Hello value) {
        return value.jitLargeIntField + value.jitIntField;
    }

    public static int jitCatchNullField() {
        try { return jitReadInt(null); }
        catch (NullPointerException expected) { return 42; }
    }

    public static int jitDivide(int divisor) {
        return 42 / divisor;
    }

    public static int jitCallInt(int x, int y) { return jitArithmetic(x, y); }
    public static long jitRangeTarget(int a, int b, int c, int d, int e, int f, int g, long wide, double real, Object value) {
        System.gc();
        return a == 11 && b == 22 && c == 33 && d == 44 && e == 55 && f == 66 && g == 77 &&
            real == -1234.5 && value != null ? wide : -1L;
    }
    public static long jitRangeCall(int a, int b, int c, int d, int e, int f, int g, long wide, double real, Object value) {
        return jitRangeTarget(a, b, c, d, e, f, g, wide, real, value);
    }
    public static int jitCallNative() { return hostPageSize(); }
    public static Object jitNativeReference(Object value) {
        Object result = nativeReferenceIdentity(value);
        jitVoidGc();
        return result;
    }
    public static int jitCallReordered(int x, int y) { return jitArithmetic(y, x); }
    public static int jitCallDuplicated(int x) { return jitArithmetic(x, x); }
    public static Object jitMixedPermutationTarget(Object v, long tag, double real) {
        return tag == 0x12345678abcdef01L && real == -123.25 ? v : null;
    }
    public static Object jitMixedPermutation(long tag, double real, Object v) {
        return jitMixedPermutationTarget(v, tag, real);
    }
    public static Object jitMixedDropped(Object ignored, Object v, long tag, double real) {
        return jitMixedPermutationTarget(v, tag, real);
    }
    public static int jitCallDropped(int x, int ignored) { return jitArithmetic(x, 7); }
    public static Object jitCallRefTarget(Object value) { return value; }
    public static Object jitCallRef(Object value) { return jitCallRefTarget(value); }
    public static Object jitMixedTarget(int first, Object value, int middle, Object other, int last) {
        return first == 17 && middle == 29 && last == 43 && value == other ? value : null;
    }
    public static Object jitCallMixed(int first, Object value, int middle, Object other, int last) {
        return jitMixedTarget(first, value, middle, other, last);
    }
    public static Object jitGcTarget(Object value) { System.gc(); return value; }
    public static void jitVoidGc() { System.gc(); }
    public static Object jitComposedGc(Object a, Object b) {
        Object first = jitGcTarget(a);
        jitVoidGc();
        Object second = jitGcTarget(b);
        return first != null ? first : second;
    }
    public static Object jitVoidValue;
    public static void jitVoidTarget(Object v) { jitVoidValue = v; }
    public static void jitVoidComposed(Object a, Object b) { jitVoidTarget(a); jitVoidTarget(b); }
    public static Object jitCallGc(Object value) { return jitGcTarget(value); }
    public static int jitCallDivide(int value) { return jitDivide(value); }
    public static int jitCatchCall() {
        try { return jitCallDivide(0); }
        catch (ArithmeticException expected) { return 42; }
    }

    public static int nativeRoundTrip() {
        return hostPageSize() == 16384 ? 42 : -1;
    }

    public static int nativeStackPcsRoundTrip() {
        long integer = nativePackedIntegerStack(
                10, 11, 12, 13, 14, 15, 0x10203040,
                0x1122334455667788L, 0x50607080, 0x12345678);
        long floating = nativePackedFloatingStack(
                1.0f, 2.0f, 3.0f, 4.0f, 5.0f, 6.0f, 7.0f, 8.0f,
                9.5f, 10.25);
        Object marker = new Object();
        long reference = nativePackedReferenceStack(
                20, 21, 22, 23, 24, 25, 0x23456701, marker, 0x34567812);
        long narrow = nativePackedNarrowStack(
                30, 31, 32, 33, 34, 35, true, (byte) 0x81, (char) 0xabcd,
                (short) 0x8765, 0x45678923, 0x2233445566778899L);
        if (integer != 0x13579bdf2468ace0L) return -10;
        if (floating != 0x02468ace13579bdfL) return -11;
        if (reference != 0x55aa55aa33cc33ccL) return -12;
        if (narrow != 0x1122aabb3344ccddL) return -13;
        return 42;
    }

    public static int runtimeNativeArraycopy() {
        int[] source = new int[] { 19, 23 };
        int[] destination = new int[2];
        System.arraycopy(source, 0, destination, 0, source.length);
        return destination[0] + destination[1];
    }

    public static long jitSpecializedIntrinsics(int i, long j, float f, double d, String value) {
        long result = 17;
        result = result * 31 + (Double.isNaN(d) ? 1 : 0);
        result = result * 31 + (Float.isNaN(f) ? 1 : 0);
        result = result * 31 + Integer.compare(i, -i);
        result = result * 31 + Integer.rotateRight(i, j > 0 ? 7 : 19);
        result = result * 31 + Integer.rotateLeft(i, j > 0 ? 11 : 23);
        result = result * 31 + Integer.signum(i);
        result = result * 31 + Long.compare(j, -j);
        result = result * 31 + Long.rotateRight(j, i > 0 ? 13 : 29);
        result = result * 31 + Long.rotateLeft(j, i > 0 ? 17 : 31);
        result = result * 31 + Long.signum(j);
        result = result * 31 + Double.doubleToRawLongBits(Math.abs(d));
        result = result * 31 + Float.floatToRawIntBits(Math.abs(f));
        result = result * 31 + Math.abs(j);
        result = result * 31 + Math.abs(i);
        result = result * 31 + Double.doubleToRawLongBits(Math.min(d, -d));
        result = result * 31 + Float.floatToRawIntBits(Math.min(f, -f));
        result = result * 31 + Math.min(j, -j);
        result = result * 31 + Math.min(i, -i);
        result = result * 31 + Double.doubleToRawLongBits(Math.max(d, -d));
        result = result * 31 + Float.floatToRawIntBits(Math.max(f, -f));
        result = result * 31 + Math.max(j, -j);
        result = result * 31 + Math.max(i, -i);
        result = result * 31 + value.charAt((i & Integer.MAX_VALUE) % value.length());
        result = result * 31 + (value.isEmpty() ? 1 : 0);
        result = result * 31 + value.length();
        java.lang.invoke.VarHandle.fullFence();
        java.lang.invoke.VarHandle.acquireFence();
        java.lang.invoke.VarHandle.releaseFence();
        java.lang.invoke.VarHandle.loadLoadFence();
        java.lang.invoke.VarHandle.storeStoreFence();
        return result;
    }

    public static char jitSpecializedStringCharAt(String value, int index) {
        return value.charAt(index);
    }

    public static long jitStringRelations(
            String value, String needle, Object other, int codePoint, int fromIndex) {
        long result = value.compareTo(needle);
        result = result * 31 + (value.equals(other) ? 1 : 0);
        result = result * 31 + value.indexOf(codePoint);
        result = result * 31 + value.indexOf(codePoint, fromIndex);
        result = result * 31 + value.indexOf(needle);
        result = result * 31 + value.indexOf(needle, fromIndex);
        return result;
    }

    public static String jitStringBuilder(
            Object object, String string, CharSequence sequence, char[] chars,
            boolean flag, char character, int integer, long wide, float single, double real) {
        StringBuilder builder = new StringBuilder();
        builder.append(object);
        builder.append(string);
        builder.append(sequence);
        builder.append(chars);
        builder.append(flag);
        builder.append(character);
        builder.append(integer);
        builder.append(wide);
        builder.append(single);
        builder.append(real);
        int length = builder.length();
        builder.append(':').append(length);
        return builder.toString();
    }

    public static String jitStringBuffer(String value) {
        StringBuffer buffer = new StringBuffer();
        buffer.append(value);
        int length = buffer.length();
        buffer.append(':').append(length);
        return buffer.toString();
    }
}

class JitConstructorParent {
    public static boolean jitParentThrow;
    public int jitParentMarker;
    public Object jitParentReceiver;

    public JitConstructorParent() {
        System.gc();
        jitParentMarker = 29;
        jitParentReceiver = this;
        if (jitParentThrow) throw new IllegalArgumentException("parent constructor");
    }
}

class JitFinalReference extends JitConstructorParent {
    public final Object payload;
    public final int small = -7;
    public final int medium = -1234;
    public final int full = 0x12345678;
    public final int high = 0x12340000;
    public final long wideSmall = -12345L;
    public final long wideMedium = -123456789L;
    public final long wideFull = 0x123456789abcdefL;
    public final long wideHigh = 0x1234000000000000L;
    public final float floatZero = -0.0f;
    public final double doubleZero = -0.0;
    public final Object nullValue = null;
    public final int computed;
    public JitFinalReference(Object value, int iterations) {
        int accumulator = value == null ? 7 : 19;
        int limit = value == null ? 0 : iterations;
        for (int i = 1; i <= limit; ++i) accumulator = (accumulator * 31) ^ i;
        computed = accumulator;
        payload = value;
    }
}

// Compilation-only symbol: deliberately excluded from acceptance DEX inputs.
class JitMissingType {
    static int number; static Object object;
    static int call() { return 19; }
    static Object reference(Object value) { return value; }
}

interface JitCallable {
    int nativeToken();
    long Aa(int a, int b, int c, int d, int e, int f, int g, long wide, double real, Object value);
    long BB(int a, int b, int c, int d, int e, int f, int g, long wide, double real, Object value);
    int Aa();
    int BB();
    long range(int a, int b, int c, int d, int e, int f, int g, long wide, double real, Object value);
    default int defaultValue() { return value() + 101; }
    int value();
    Object reference();
}
class JitVirtualBase implements JitCallable {
    public native int nativeToken();
    public long Aa(int a, int b, int c, int d, int e, int f, int g, long wide, double real, Object value) {
        return Hello.jitRangeTarget(a, b, c, d, e, f, g, wide, real, value);
    }
    public long BB(int a, int b, int c, int d, int e, int f, int g, long wide, double real, Object value) {
        return -Hello.jitRangeTarget(a, b, c, d, e, f, g, wide, real, value);
    }
    public int Aa() { return 401; }
    public int BB() { return 809; }
    public long wideSpeculative(int a,int b,int c,int d,int e,int f,int g,long wide,double real,Object value) { return wide; }
    public long range(int a,int b,int c,int d,int e,int f,int g,long wide,double real,Object value) {
        return Hello.jitRangeTarget(a,b,c,d,e,f,g,wide,real,value);
    }
    public int number;
    public Object object;
    public int value() { return number; }
    public Object reference() { return object; }
}

class JitColdInitialization {
    static { Hello.jitInitializationCount++; }
    static int read() { return 41; }
}

class JitColdStatic {
    static int value;
    static { Hello.jitStaticInitializationCount++; value = 93; }
}

class JitFailedStaticRead {
    static long value;
    static { Hello.jitFailedStaticReadCount++; int unused = 1 / Hello.jitInitializationZero; }
}
class JitFailedStaticWrite {
    static long value;
    static { Hello.jitFailedStaticWriteCount++; int unused = 1 / Hello.jitInitializationZero; }
}

class JitColdMoving {
    static Object value;
    static {
        Hello.jitColdMovingCount++;
        synchronized (Hello.jitConcurrentInitializationLock) { value = null; }
    }
}

class JitColdLong {
    static long value;
    static { Hello.jitColdLongCount++; value = -1; }
}
class JitColdDouble {
    static double value;
    static { Hello.jitColdDoubleCount++; value = -2.0; }
}
class JitColdReference {
    static Object value;
    static { Hello.jitColdReferenceCount++; value = new byte[65536]; }
}

class JitRecursiveInitialization {
    static int published;
    static {
        Hello.jitRecursiveInitializationCount++;
        Hello.jitRecursiveInitializationObserved = Hello.jitRecursiveInitialization();
        published = 73;
    }
    static int read() { return published; }
}

class JitConcurrentInitialization {
    static int published;
    static {
        Hello.jitConcurrentInitializationCount++;
        synchronized (Hello.jitConcurrentInitializationLock) { published = 73; }
    }
    static int read() { return published; }
}

class JitConcurrentFailedInitialization {
    static {
        Hello.jitConcurrentFailedInitializationCount++;
        synchronized (Hello.jitConcurrentInitializationLock) {
            int unused = 1 / Hello.jitInitializationZero;
        }
    }
    static int read() { return 41; }
}

class JitFailedInitialization {
    static {
        Hello.jitFailedInitializationCount++;
        int unused = 1 / Hello.jitInitializationZero;
    }
    static int read() { return 41; }
}

class JitVirtualChild extends JitVirtualBase {
    public int superValue() { return super.value(); }
    public Object superReference() { return super.reference(); }
    public long superRange(int a, int b, int c, int d, int e, int f, int g, long wide, double real, Object value) {
        return super.range(a, b, c, d, e, f, g, wide, real, value);
    }
    @Override public native int nativeToken();
    @Override public long wideSpeculative(int a,int b,int c,int d,int e,int f,int g,long wide,double real,Object value) {
        if (value != this) return -3L;
        return -Hello.jitRangeTarget(a,b,c,d,e,f,g,wide,real,value);
    }
    @Override public long range(int a,int b,int c,int d,int e,int f,int g,long wide,double real,Object value) {
        return -Hello.jitRangeTarget(a,b,c,d,e,f,g,wide,real,value);
    }
    public int alternative;
    public Object alternativeObject;
    @Override public int value() { return alternative; }
    @Override public Object reference() { return alternativeObject; }
}
