import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.FieldVisitor;
import org.objectweb.asm.MethodVisitor;
import org.objectweb.asm.Opcodes;

/**
 * Writes signature-only class files for every class in the pinned platform
 * DEX files, so Java owners compile against the exact hidden API: access
 * flags, generic signatures, inner classes, thrown exceptions and constant
 * values come from the DEX itself. Method bodies are omitted; the output is a
 * javac bootclasspath, never runtime code.
 *
 * <p>Usage: HiddenApiSignatures OUTPUT_DIR INPUT.jar...; earlier inputs win
 * when two jars define the same class (bootclasspath order).</p>
 */
public final class HiddenApiSignatures {
    private static final int NO_INDEX = -1;

    public static void main(String[] args) throws IOException {
        if (args.length < 2) {
            System.err.println("usage: HiddenApiSignatures OUTPUT_DIR INPUT.jar...");
            System.exit(64);
        }
        Path output = Paths.get(args[0]);
        List<Dex> dexes = new ArrayList<>();
        for (int i = 1; i < args.length; i++) {
            try (ZipFile jar = new ZipFile(args[i])) {
                List<String> names = new ArrayList<>();
                for (ZipEntry entry : java.util.Collections.list(jar.entries())) {
                    String name = entry.getName();
                    if (name.matches("classes[0-9]*\\.dex")) names.add(name);
                }
                names.sort((a, b) -> Integer.compare(index(a), index(b)));
                if (names.isEmpty()) throw new IOException("no DEX in " + args[i]);
                for (String name : names) {
                    dexes.add(new Dex(read(jar.getInputStream(jar.getEntry(name)))));
                }
            }
        }
        // R8 drops MemberClasses; rebuild each outer class's InnerClasses
        // entries from its nested classes' own InnerClass annotations.
        Map<String, List<String[]>> members = new HashMap<>();
        Set<String> seen = new HashSet<>();
        for (Dex dex : dexes) dex.collectNested(seen, members);
        Set<String> written = new HashSet<>();
        int total = 0;
        for (Dex dex : dexes) total += dex.writeClasses(output, written, members);
        System.out.println("hidden-api-signatures: classes=" + total + " output=" + output);
    }

    private static int index(String name) {
        String digits = name.substring("classes".length(), name.length() - ".dex".length());
        return digits.isEmpty() ? 1 : Integer.parseInt(digits);
    }

    private static byte[] read(InputStream input) throws IOException {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        byte[] buffer = new byte[1 << 16];
        for (int n; (n = input.read(buffer)) > 0; ) bytes.write(buffer, 0, n);
        input.close();
        return bytes.toByteArray();
    }

    /** A cursor over little-endian DEX data. */
    private static final class Cursor {
        final byte[] data;
        int position;
        Cursor(byte[] data, int position) { this.data = data; this.position = position; }
        int u1() { return data[position++] & 0xff; }
        int u2() { return u1() | (u1() << 8); }
        int u4() { return u2() | (u2() << 16); }
        int uleb() {
            int result = 0;
            for (int shift = 0; ; shift += 7) {
                int b = u1();
                result |= (b & 0x7f) << shift;
                if ((b & 0x80) == 0) return result;
            }
        }
        long sized(int size, boolean signExtend) {
            long value = 0;
            for (int i = 0; i < size; i++) value |= ((long) u1()) << (8 * i);
            if (signExtend && size < 8) {
                int shift = 64 - 8 * size;
                value = (value << shift) >> shift;
            }
            return value;
        }
    }

    private static final class Annotations {
        String signature;
        List<String> throwsTypes = new ArrayList<>();
        // InnerClass: simple name (null when anonymous) and access flags.
        boolean inner;
        String innerName;
        int innerAccess;
        String enclosingClass;
        List<String> memberClasses = new ArrayList<>();
    }

    private static final class Dex {
        final byte[] data;
        final int[] stringOffsets;
        final int[] typeIds;
        final int protoIdsOff;
        final int fieldIdsOff;
        final int methodIdsOff;
        final int classDefsSize;
        final int classDefsOff;
        final Map<Integer, String> strings = new HashMap<>();

        Dex(byte[] data) throws IOException {
            this.data = data;
            if (data.length < 0x70 || data[0] != 'd' || data[1] != 'e' || data[2] != 'x') {
                throw new IOException("not a DEX file");
            }
            Cursor header = new Cursor(data, 0x38);
            int stringIdsSize = header.u4();
            int stringIdsOff = header.u4();
            int typeIdsSize = header.u4();
            int typeIdsOff = header.u4();
            header.u4();
            protoIdsOff = header.u4();
            header.u4();
            fieldIdsOff = header.u4();
            header.u4();
            methodIdsOff = header.u4();
            classDefsSize = header.u4();
            classDefsOff = header.u4();
            stringOffsets = new int[stringIdsSize];
            Cursor strings = new Cursor(data, stringIdsOff);
            for (int i = 0; i < stringIdsSize; i++) stringOffsets[i] = strings.u4();
            typeIds = new int[typeIdsSize];
            Cursor types = new Cursor(data, typeIdsOff);
            for (int i = 0; i < typeIdsSize; i++) typeIds[i] = types.u4();
        }

        String string(int index) {
            return strings.computeIfAbsent(index, i -> {
                Cursor cursor = new Cursor(data, stringOffsets[i]);
                cursor.uleb();
                int start = cursor.position;
                int end = start;
                while (data[end] != 0) end++;
                return mutf8(data, start, end);
            });
        }

        String type(int index) { return string(typeIds[index]); }

        /** Internal name for an object descriptor "Lpkg/Name;". */
        static String internal(String descriptor) {
            return descriptor.substring(1, descriptor.length() - 1);
        }

        String methodName(int methodIdx) {
            return string(new Cursor(data, methodIdsOff + methodIdx * 8 + 4).u4());
        }

        String methodDescriptor(int methodIdx) {
            int protoIdx = new Cursor(data, methodIdsOff + methodIdx * 8 + 2).u2();
            Cursor proto = new Cursor(data, protoIdsOff + protoIdx * 12);
            proto.u4();
            String returnType = type(proto.u4());
            int parametersOff = proto.u4();
            StringBuilder descriptor = new StringBuilder("(");
            if (parametersOff != 0) {
                Cursor list = new Cursor(data, parametersOff);
                int size = list.u4();
                for (int i = 0; i < size; i++) descriptor.append(type(list.u2()));
            }
            return descriptor.append(')').append(returnType).toString();
        }

        String fieldName(int fieldIdx) {
            return string(new Cursor(data, fieldIdsOff + fieldIdx * 8 + 4).u4());
        }

        String fieldType(int fieldIdx) {
            return type(new Cursor(data, fieldIdsOff + fieldIdx * 8 + 2).u2());
        }

        void collectNested(Set<String> seen, Map<String, List<String[]>> members) {
            for (int i = 0; i < classDefsSize; i++) {
                Cursor def = new Cursor(data, classDefsOff + i * 32);
                String name = internal(type(def.u4()));
                def.u4();
                def.u4();
                def.u4();
                def.u4();
                int annotationsOff = def.u4();
                int classDataOff = def.u4();
                if (!seen.add(name)) continue;
                int classSet = annotationsOff == 0 ? 0 : new Cursor(data, annotationsOff).u4();
                Annotations annotations = classSet == 0 ? new Annotations() : annotationSet(classSet);
                if (annotations.inner) {
                    if (annotations.innerName == null || annotations.enclosingClass == null) continue;
                    members.computeIfAbsent(annotations.enclosingClass, k -> new ArrayList<>())
                            .add(new String[] {name, annotations.innerName,
                                    Integer.toString(annotations.innerAccess & 0xffff),
                                    annotations.enclosingClass});
                    continue;
                }
                // R8 also strips InnerClass: derive a named member class from
                // its binary name; it is an inner (non-static) class only when
                // it keeps the synthetic outer-instance field.
                int dollar = name.lastIndexOf('$');
                if (dollar <= 0 || dollar == name.length() - 1) continue;
                String simple = name.substring(dollar + 1);
                if (!Character.isJavaIdentifierStart(simple.charAt(0))) continue;
                int access = new Cursor(data, classDefsOff + i * 32 + 4).u4() & 0x761f;
                if (!hasOuterInstance(classDataOff)) access |= Opcodes.ACC_STATIC;
                String outer = name.substring(0, dollar);
                String[] entry = {name, simple, Integer.toString(access), outer};
                members.computeIfAbsent(outer, k -> new ArrayList<>()).add(entry);
                members.computeIfAbsent(name, k -> new ArrayList<>()).add(entry);
            }
        }

        boolean hasOuterInstance(int classDataOff) {
            if (classDataOff == 0) return false;
            Cursor classData = new Cursor(data, classDataOff);
            int staticFields = classData.uleb();
            int instanceFields = classData.uleb();
            classData.uleb();
            classData.uleb();
            int fieldIdx = 0;
            for (int i = 0; i < staticFields + instanceFields; i++) {
                if (i == staticFields) fieldIdx = 0;
                fieldIdx += classData.uleb();
                classData.uleb();
                if (i >= staticFields && fieldName(fieldIdx).startsWith("this$")) return true;
            }
            return false;
        }

        int writeClasses(Path output, Set<String> written, Map<String, List<String[]>> members)
                throws IOException {
            int count = 0;
            for (int i = 0; i < classDefsSize; i++) {
                Cursor def = new Cursor(data, classDefsOff + i * 32);
                String name = internal(type(def.u4()));
                int access = def.u4();
                int superIdx = def.u4();
                int interfacesOff = def.u4();
                def.u4(); // source file
                int annotationsOff = def.u4();
                int classDataOff = def.u4();
                int staticValuesOff = def.u4();
                if (!written.add(name)) continue;
                Path file = output.resolve(name + ".class");
                Files.createDirectories(file.getParent());
                Files.write(file, writeClass(name, access, superIdx, interfacesOff,
                        annotationsOff, classDataOff, staticValuesOff,
                        members.getOrDefault(name, java.util.Collections.emptyList())));
                count++;
            }
            return count;
        }

        byte[] writeClass(String name, int access, int superIdx, int interfacesOff,
                int annotationsOff, int classDataOff, int staticValuesOff,
                List<String[]> nested) {
            Map<Integer, Annotations> fieldAnnotations = new HashMap<>();
            Map<Integer, Annotations> methodAnnotations = new HashMap<>();
            Annotations classAnnotations = new Annotations();
            if (annotationsOff != 0) {
                Cursor directory = new Cursor(data, annotationsOff);
                int classSet = directory.u4();
                int fields = directory.u4();
                int methods = directory.u4();
                directory.u4();
                if (classSet != 0) classAnnotations = annotationSet(classSet);
                for (int i = 0; i < fields; i++) {
                    int idx = directory.u4();
                    fieldAnnotations.put(idx, annotationSet(directory.u4()));
                }
                for (int i = 0; i < methods; i++) {
                    int idx = directory.u4();
                    methodAnnotations.put(idx, annotationSet(directory.u4()));
                }
            }
            String superName = superIdx == NO_INDEX ? null : internal(type(superIdx));
            List<String> interfaces = new ArrayList<>();
            if (interfacesOff != 0) {
                Cursor list = new Cursor(data, interfacesOff);
                int size = list.u4();
                for (int i = 0; i < size; i++) interfaces.add(internal(type(list.u2())));
            }
            int classAccess = access & 0x7631;
            if (classAnnotations.inner) {
                // The class file keeps only public/package access for nested
                // classes; the real flags live in InnerClasses.
                if ((classAnnotations.innerAccess & Opcodes.ACC_PROTECTED) != 0) {
                    classAccess |= Opcodes.ACC_PUBLIC;
                }
                classAccess &= ~(Opcodes.ACC_PRIVATE | Opcodes.ACC_PROTECTED | Opcodes.ACC_STATIC);
            }
            if ((classAccess & Opcodes.ACC_INTERFACE) == 0) classAccess |= Opcodes.ACC_SUPER;
            ClassWriter writer = new ClassWriter(0);
            writer.visit(Opcodes.V1_8, classAccess, name, classAnnotations.signature, superName,
                    interfaces.toArray(new String[0]));
            if (classAnnotations.inner) {
                writer.visitInnerClass(name, classAnnotations.enclosingClass,
                        classAnnotations.innerName, classAnnotations.innerAccess & 0xffff);
            }
            // Entries for this class's members, and for this class itself
            // when its nesting was derived from its binary name.
            for (String[] member : nested) {
                if (classAnnotations.inner && member[0].equals(name)) continue;
                writer.visitInnerClass(member[0], member[3], member[1], Integer.parseInt(member[2]));
            }
            if (classDataOff != 0) {
                Cursor classData = new Cursor(data, classDataOff);
                int staticFields = classData.uleb();
                int instanceFields = classData.uleb();
                int directMethods = classData.uleb();
                int virtualMethods = classData.uleb();
                List<Object> constants = staticValuesOff == 0 ? new ArrayList<>()
                        : encodedArray(new Cursor(data, staticValuesOff));
                int fieldIdx = 0;
                for (int i = 0; i < staticFields + instanceFields; i++) {
                    if (i == staticFields) fieldIdx = 0;
                    fieldIdx += classData.uleb();
                    int fieldAccess = classData.uleb();
                    Object constant = null;
                    if (i < staticFields && i < constants.size()
                            && (fieldAccess & Opcodes.ACC_FINAL) != 0) {
                        constant = constantFor(fieldType(fieldIdx), constants.get(i));
                    }
                    Annotations annotations = fieldAnnotations.get(fieldIdx);
                    FieldVisitor field = writer.visitField(fieldAccess & 0xffff,
                            fieldName(fieldIdx), fieldType(fieldIdx),
                            annotations == null ? null : annotations.signature, constant);
                    field.visitEnd();
                }
                int methodIdx = 0;
                for (int i = 0; i < directMethods + virtualMethods; i++) {
                    if (i == directMethods) methodIdx = 0;
                    methodIdx += classData.uleb();
                    int methodAccess = classData.uleb();
                    classData.uleb(); // code offset
                    Annotations annotations = methodAnnotations.get(methodIdx);
                    String[] exceptions = annotations == null || annotations.throwsTypes.isEmpty()
                            ? null : annotations.throwsTypes.toArray(new String[0]);
                    MethodVisitor method = writer.visitMethod(methodAccess & 0xffff,
                            methodName(methodIdx), methodDescriptor(methodIdx),
                            annotations == null ? null : annotations.signature, exceptions);
                    method.visitEnd();
                }
            }
            writer.visitEnd();
            return writer.toByteArray();
        }

        static Object constantFor(String type, Object value) {
            if (value == null) return null;
            switch (type) {
                case "Z": return (value instanceof Boolean && (Boolean) value) ? 1 : 0;
                case "B": case "S": case "C": case "I":
                    return value instanceof Number ? ((Number) value).intValue() : null;
                case "J": return value instanceof Number ? ((Number) value).longValue() : null;
                case "F": return value instanceof Float ? value : null;
                case "D": return value instanceof Double ? value : null;
                case "Ljava/lang/String;": return value instanceof String ? value : null;
                default: return null;
            }
        }

        Annotations annotationSet(int offset) {
            Annotations result = new Annotations();
            Cursor set = new Cursor(data, offset);
            int size = set.u4();
            for (int i = 0; i < size; i++) {
                Cursor item = new Cursor(data, set.u4());
                item.u1(); // visibility
                String annotationType = type(item.uleb());
                Map<String, Object> elements = annotationElements(item);
                switch (annotationType) {
                    case "Ldalvik/annotation/Signature;": {
                        StringBuilder signature = new StringBuilder();
                        for (Object part : (List<?>) elements.get("value")) signature.append(part);
                        result.signature = signature.toString();
                        break;
                    }
                    case "Ldalvik/annotation/Throws;":
                        for (Object thrown : (List<?>) elements.get("value")) {
                            result.throwsTypes.add(internal((String) ((TypeRef) thrown).descriptor));
                        }
                        break;
                    case "Ldalvik/annotation/InnerClass;":
                        result.inner = true;
                        result.innerName = (String) elements.get("name");
                        result.innerAccess = ((Number) elements.get("accessFlags")).intValue();
                        break;
                    case "Ldalvik/annotation/EnclosingClass;":
                        result.enclosingClass = internal(((TypeRef) elements.get("value")).descriptor);
                        break;
                    case "Ldalvik/annotation/MemberClasses;":
                        for (Object member : (List<?>) elements.get("value")) {
                            result.memberClasses.add(internal(((TypeRef) member).descriptor));
                        }
                        break;
                    default:
                        break;
                }
            }
            return result;
        }

        Map<String, Object> annotationElements(Cursor cursor) {
            int size = cursor.uleb();
            Map<String, Object> elements = new HashMap<>();
            for (int i = 0; i < size; i++) {
                String name = string(cursor.uleb());
                elements.put(name, encodedValue(cursor));
            }
            return elements;
        }

        List<Object> encodedArray(Cursor cursor) {
            int size = cursor.uleb();
            List<Object> values = new ArrayList<>(size);
            for (int i = 0; i < size; i++) values.add(encodedValue(cursor));
            return values;
        }

        Object encodedValue(Cursor cursor) {
            int header = cursor.u1();
            int valueType = header & 0x1f;
            int arg = header >> 5;
            switch (valueType) {
                case 0x00: return (byte) cursor.sized(1, true);
                case 0x02: return (short) cursor.sized(arg + 1, true);
                case 0x03: return (char) cursor.sized(arg + 1, false);
                case 0x04: return (int) cursor.sized(arg + 1, true);
                case 0x06: return cursor.sized(arg + 1, true);
                case 0x10: return Float.intBitsToFloat((int) (cursor.sized(arg + 1, false) << (8 * (3 - arg))));
                case 0x11: return Double.longBitsToDouble(cursor.sized(arg + 1, false) << (8 * (7 - arg)));
                case 0x17: return string((int) cursor.sized(arg + 1, false));
                case 0x18: return new TypeRef(type((int) cursor.sized(arg + 1, false)));
                case 0x15: case 0x16: case 0x19: case 0x1a: case 0x1b:
                    cursor.sized(arg + 1, false);
                    return null;
                case 0x1c: return encodedArray(cursor);
                case 0x1d:
                    cursor.uleb();
                    annotationElements(cursor);
                    return null;
                case 0x1e: return null;
                case 0x1f: return arg != 0;
                default: throw new IllegalStateException("bad encoded value type " + valueType);
            }
        }
    }

    private static final class TypeRef {
        final String descriptor;
        TypeRef(String descriptor) { this.descriptor = descriptor; }
    }

    private static String mutf8(byte[] data, int start, int end) {
        StringBuilder out = new StringBuilder(end - start);
        for (int i = start; i < end; ) {
            int a = data[i++] & 0xff;
            if (a < 0x80) {
                out.append((char) a);
            } else if ((a & 0xe0) == 0xc0) {
                int b = data[i++] & 0x3f;
                out.append((char) (((a & 0x1f) << 6) | b));
            } else {
                int b = data[i++] & 0x3f;
                int c = data[i++] & 0x3f;
                out.append((char) (((a & 0x0f) << 12) | (b << 6) | c));
            }
        }
        return out.toString();
    }
}
