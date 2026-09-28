import java.nio.file.Files;
import java.nio.file.Path;
import jdk.internal.org.objectweb.asm.ClassWriter;
import jdk.internal.org.objectweb.asm.MethodVisitor;
import jdk.internal.org.objectweb.asm.Opcodes;
import jdk.internal.org.objectweb.asm.Type;

/** Emits direct hidden-API calls so D8/ART see the same Unsafe invokes as framework code. */
public final class GenerateJitUnsafe implements Opcodes {
    private static final String OWNER = "dev/aim/probe/JitUnsafe";
    private static final String UNSAFE = "jdk/internal/misc/Unsafe";
    private static final String UNSAFE_DESCRIPTOR = "L" + UNSAFE + ";";
    private static final String STRING_OWNER = "java/lang/JitStringHidden";
    private static final String STRING_FACTORY = "java/lang/StringFactory";
    private static final String MATH_OWNER = "dev/aim/probe/JitMathDirect";
    private static final String MEMORY_OWNER = "libcore/io/JitMemoryDirect";
    private static final String REFERENCE_OWNER = "java/lang/ref/JitReferenceDirect";
    private static final String BOXING_OWNER = "java/lang/JitBoxingDirect";

    private static final String[][] METHODS = {
        {"getInt", "getInt", "(Ljava/lang/Object;J)I"},
        {"getIntAbsolute", "getInt", "(J)I"},
        {"getIntVolatile", "getIntVolatile", "(Ljava/lang/Object;J)I"},
        {"getIntAcquire", "getIntAcquire", "(Ljava/lang/Object;J)I"},
        {"getLong", "getLong", "(Ljava/lang/Object;J)J"},
        {"getLongVolatile", "getLongVolatile", "(Ljava/lang/Object;J)J"},
        {"getLongAcquire", "getLongAcquire", "(Ljava/lang/Object;J)J"},
        {"getByte", "getByte", "(Ljava/lang/Object;J)B"},
        {"getReference", "getReference", "(Ljava/lang/Object;J)Ljava/lang/Object;"},
        {"getReferenceVolatile", "getReferenceVolatile", "(Ljava/lang/Object;J)Ljava/lang/Object;"},
        {"getReferenceAcquire", "getReferenceAcquire", "(Ljava/lang/Object;J)Ljava/lang/Object;"},
        {"putInt", "putInt", "(Ljava/lang/Object;JI)V"},
        {"putIntAbsolute", "putInt", "(JI)V"},
        {"putIntRelease", "putIntRelease", "(Ljava/lang/Object;JI)V"},
        {"putIntVolatile", "putIntVolatile", "(Ljava/lang/Object;JI)V"},
        {"putLong", "putLong", "(Ljava/lang/Object;JJ)V"},
        {"putLongRelease", "putLongRelease", "(Ljava/lang/Object;JJ)V"},
        {"putLongVolatile", "putLongVolatile", "(Ljava/lang/Object;JJ)V"},
        {"putByte", "putByte", "(Ljava/lang/Object;JB)V"},
        {"putReference", "putReference", "(Ljava/lang/Object;JLjava/lang/Object;)V"},
        {"putReferenceRelease", "putReferenceRelease", "(Ljava/lang/Object;JLjava/lang/Object;)V"},
        {"putReferenceVolatile", "putReferenceVolatile", "(Ljava/lang/Object;JLjava/lang/Object;)V"},
        {"compareAndSetInt", "compareAndSetInt", "(Ljava/lang/Object;JII)Z"},
        {"compareAndSetLong", "compareAndSetLong", "(Ljava/lang/Object;JJJ)Z"},
        {"compareAndSetReference", "compareAndSetReference", "(Ljava/lang/Object;JLjava/lang/Object;Ljava/lang/Object;)Z"},
        {"getAndAddInt", "getAndAddInt", "(Ljava/lang/Object;JI)I"},
        {"getAndAddLong", "getAndAddLong", "(Ljava/lang/Object;JJ)J"},
        {"getAndSetInt", "getAndSetInt", "(Ljava/lang/Object;JI)I"},
        {"getAndSetLong", "getAndSetLong", "(Ljava/lang/Object;JJ)J"},
        {"getAndSetReference", "getAndSetReference", "(Ljava/lang/Object;JLjava/lang/Object;)Ljava/lang/Object;"},
        {"arrayBaseOffset", "arrayBaseOffset", "(Ljava/lang/Class;)I"},
        {"loadFence", "loadFence", "()V"},
        {"storeFence", "storeFence", "()V"},
        {"fullFence", "fullFence", "()V"},
    };

    private static void wrapper(ClassWriter writer, String wrapperName,
            String targetName, String targetDescriptor) {
        String wrapperDescriptor = "(" + UNSAFE_DESCRIPTOR + targetDescriptor.substring(1);
        MethodVisitor method = writer.visitMethod(
                ACC_PUBLIC | ACC_STATIC, wrapperName, wrapperDescriptor, null, null);
        method.visitCode();
        Type[] arguments = Type.getArgumentTypes(wrapperDescriptor);
        int local = 0;
        for (Type argument : arguments) {
            method.visitVarInsn(argument.getOpcode(ILOAD), local);
            local += argument.getSize();
        }
        method.visitMethodInsn(INVOKEVIRTUAL, UNSAFE, targetName, targetDescriptor, false);
        method.visitInsn(Type.getReturnType(targetDescriptor).getOpcode(IRETURN));
        method.visitMaxs(0, 0);
        method.visitEnd();
    }

    private static void hiddenStringWrappers(Path outputRoot) throws Exception {
        ClassWriter writer = new ClassWriter(ClassWriter.COMPUTE_FRAMES | ClassWriter.COMPUTE_MAXS);
        writer.visit(V1_8, ACC_PUBLIC | ACC_FINAL | ACC_SUPER,
                STRING_OWNER, null, "java/lang/Object", null);

        MethodVisitor getChars = writer.visitMethod(ACC_PUBLIC | ACC_STATIC,
                "getCharsNoCheck", "(Ljava/lang/String;II[CI)V", null, null);
        getChars.visitCode();
        getChars.visitVarInsn(ALOAD, 0);
        getChars.visitVarInsn(ILOAD, 1);
        getChars.visitVarInsn(ILOAD, 2);
        getChars.visitVarInsn(ALOAD, 3);
        getChars.visitVarInsn(ILOAD, 4);
        getChars.visitMethodInsn(INVOKEVIRTUAL, "java/lang/String", "getCharsNoCheck",
                "(II[CI)V", false);
        getChars.visitInsn(RETURN);
        getChars.visitMaxs(0, 0);
        getChars.visitEnd();

        staticWrapper(writer, "newStringFromBytes", STRING_FACTORY,
                "newStringFromBytes", "([BIII)Ljava/lang/String;");
        staticWrapper(writer, "newStringFromChars", STRING_FACTORY,
                "newStringFromChars", "(II[C)Ljava/lang/String;");
        staticWrapper(writer, "newStringFromString", STRING_FACTORY,
                "newStringFromString", "(Ljava/lang/String;)Ljava/lang/String;");
        writer.visitEnd();
        Path output = outputRoot.resolve(STRING_OWNER + ".class");
        Files.createDirectories(output.getParent());
        Files.write(output, writer.toByteArray());
    }

    private static void directMathWrappers(Path outputRoot) throws Exception {
        ClassWriter writer = new ClassWriter(ClassWriter.COMPUTE_FRAMES | ClassWriter.COMPUTE_MAXS);
        writer.visit(V1_8, ACC_PUBLIC | ACC_FINAL | ACC_SUPER,
                MATH_OWNER, null, "java/lang/Object", null);
        staticWrapper(writer, "multiplyHigh", "java/lang/Math", "multiplyHigh", "(JJ)J");
        writer.visitEnd();
        Path output = outputRoot.resolve(MATH_OWNER + ".class");
        Files.createDirectories(output.getParent());
        Files.write(output, writer.toByteArray());
    }

    private static void directMemoryWrappers(Path outputRoot) throws Exception {
        ClassWriter writer = new ClassWriter(ClassWriter.COMPUTE_FRAMES | ClassWriter.COMPUTE_MAXS);
        writer.visit(V1_8, ACC_PUBLIC | ACC_FINAL | ACC_SUPER,
                MEMORY_OWNER, null, "java/lang/Object", null);
        staticWrapper(writer, "peekByte", "libcore/io/Memory", "peekByte", "(J)B");
        staticWrapper(writer, "peekShortNative", "libcore/io/Memory", "peekShort", "(JZ)S");
        staticWrapper(writer, "peekIntNative", "libcore/io/Memory", "peekInt", "(JZ)I");
        staticWrapper(writer, "peekLongNative", "libcore/io/Memory", "peekLong", "(JZ)J");
        staticWrapper(writer, "pokeByte", "libcore/io/Memory", "pokeByte", "(JB)V");
        staticWrapper(writer, "pokeShortNative", "libcore/io/Memory", "pokeShort", "(JSZ)V");
        staticWrapper(writer, "pokeIntNative", "libcore/io/Memory", "pokeInt", "(JIZ)V");
        staticWrapper(writer, "pokeLongNative", "libcore/io/Memory", "pokeLong", "(JJZ)V");
        writer.visitEnd();
        Path output = outputRoot.resolve(MEMORY_OWNER + ".class");
        Files.createDirectories(output.getParent());
        Files.write(output, writer.toByteArray());
    }

    private static void directReferenceWrappers(Path outputRoot) throws Exception {
        ClassWriter writer = new ClassWriter(ClassWriter.COMPUTE_FRAMES | ClassWriter.COMPUTE_MAXS);
        writer.visit(V1_8, ACC_PUBLIC | ACC_FINAL | ACC_SUPER,
                REFERENCE_OWNER, null, "java/lang/Object", null);
        virtualWrapper(writer, "getReferent", "java/lang/ref/Reference",
                "get", "()Ljava/lang/Object;");
        virtualWrapper(writer, "refersTo", "java/lang/ref/Reference",
                "refersTo", "(Ljava/lang/Object;)Z");
        staticWrapper(writer, "reachabilityFence", "java/lang/ref/Reference",
                "reachabilityFence", "(Ljava/lang/Object;)V");
        writer.visitEnd();
        Path output = outputRoot.resolve(REFERENCE_OWNER + ".class");
        Files.createDirectories(output.getParent());
        Files.write(output, writer.toByteArray());
    }

    private static void directBoxingWrappers(Path outputRoot) throws Exception {
        ClassWriter writer = new ClassWriter(ClassWriter.COMPUTE_FRAMES | ClassWriter.COMPUTE_MAXS);
        writer.visit(V1_8, ACC_PUBLIC | ACC_FINAL | ACC_SUPER,
                BOXING_OWNER, null, "java/lang/Object", null);
        staticWrapper(writer, "byteValueOf", "java/lang/Byte", "valueOf", "(B)Ljava/lang/Byte;");
        staticWrapper(writer, "shortValueOf", "java/lang/Short", "valueOf", "(S)Ljava/lang/Short;");
        staticWrapper(writer, "characterValueOf", "java/lang/Character", "valueOf",
                "(C)Ljava/lang/Character;");
        staticWrapper(writer, "integerValueOf", "java/lang/Integer", "valueOf",
                "(I)Ljava/lang/Integer;");
        writer.visitEnd();
        Path output = outputRoot.resolve(BOXING_OWNER + ".class");
        Files.createDirectories(output.getParent());
        Files.write(output, writer.toByteArray());
    }

    private static void virtualWrapper(ClassWriter writer, String wrapperName,
            String targetOwner, String targetName, String targetDescriptor) {
        String wrapperDescriptor = "(L" + targetOwner + ";" + targetDescriptor.substring(1);
        MethodVisitor method = writer.visitMethod(
                ACC_PUBLIC | ACC_STATIC, wrapperName, wrapperDescriptor, null, null);
        method.visitCode();
        Type[] arguments = Type.getArgumentTypes(wrapperDescriptor);
        int local = 0;
        for (Type argument : arguments) {
            method.visitVarInsn(argument.getOpcode(ILOAD), local);
            local += argument.getSize();
        }
        method.visitMethodInsn(INVOKEVIRTUAL, targetOwner, targetName, targetDescriptor, false);
        method.visitInsn(Type.getReturnType(targetDescriptor).getOpcode(IRETURN));
        method.visitMaxs(0, 0);
        method.visitEnd();
    }

    private static void staticWrapper(ClassWriter writer, String wrapperName,
            String targetOwner, String targetName, String descriptor) {
        MethodVisitor method = writer.visitMethod(
                ACC_PUBLIC | ACC_STATIC, wrapperName, descriptor, null, null);
        method.visitCode();
        Type[] arguments = Type.getArgumentTypes(descriptor);
        int local = 0;
        for (Type argument : arguments) {
            method.visitVarInsn(argument.getOpcode(ILOAD), local);
            local += argument.getSize();
        }
        method.visitMethodInsn(INVOKESTATIC, targetOwner, targetName, descriptor, false);
        method.visitInsn(Type.getReturnType(descriptor).getOpcode(IRETURN));
        method.visitMaxs(0, 0);
        method.visitEnd();
    }

    public static void main(String[] arguments) throws Exception {
        if (arguments.length != 1) throw new IllegalArgumentException("expected class output root");
        ClassWriter writer = new ClassWriter(ClassWriter.COMPUTE_FRAMES | ClassWriter.COMPUTE_MAXS);
        writer.visit(V1_8, ACC_PUBLIC | ACC_FINAL | ACC_SUPER, OWNER, null, "java/lang/Object", null);
        for (String[] method : METHODS) wrapper(writer, method[0], method[1], method[2]);
        writer.visitEnd();
        Path output = Path.of(arguments[0]).resolve(OWNER + ".class");
        Files.createDirectories(output.getParent());
        Files.write(output, writer.toByteArray());
        hiddenStringWrappers(Path.of(arguments[0]));
        directMathWrappers(Path.of(arguments[0]));
        directMemoryWrappers(Path.of(arguments[0]));
        directReferenceWrappers(Path.of(arguments[0]));
        directBoxingWrappers(Path.of(arguments[0]));
    }
}
