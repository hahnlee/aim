package dev.darwinart.runtime.compat;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;

public final class SystemCompatInputTest {
    interface Checked { void run() throws Exception; }
    static void reject(Checked operation) throws Exception {
        try {
            operation.run();
        } catch (Exception expected) {
            return;
        }
        throw new AssertionError("Invalid compatibility input accepted");
    }
    static byte[] bytes(String xml) { return xml.getBytes(StandardCharsets.UTF_8); }
    static String module(String name, String active) {
        return "<apex-info moduleName='" + name + "' isActive='" + active + "'/>";
    }
    static void inventory(Path apex, String entries) throws Exception {
        Files.write(apex.resolve("apex-info-list.xml"), bytes("<apex-info-list>" + entries + "</apex-info-list>"));
    }
    public static void main(String[] args) throws Exception {
        SystemCompatConfigReader.validate(bytes("<?xml version='1.0'?><config><unknown><child/></unknown></config>\n"));
        reject(() -> SystemCompatConfigReader.validate(bytes("<config/><config/>")));
        reject(() -> SystemCompatConfigReader.validate(bytes("<config><unknown><child/>")));
        reject(() -> SystemCompatConfigReader.validate(bytes("<wrong/>")));
        reject(() -> SystemCompatConfigReader.validate(bytes("<config/>garbage")));
        reject(() -> SystemCompatConfigReader.validate(bytes("<!DOCTYPE config [<!ENTITY e 'x'>]><config/>")));
        reject(() -> SystemCompatConfigReader.validate(new byte[2 * 1024 * 1024 + 1]));
        Path root = new File(args[0]).toPath();
        Path system = root.resolve("system");
        Path apex = root.resolve("apex");
        Files.createDirectories(system.resolve("etc/compatconfig"));
        Files.createDirectories(system.resolve("system_ext/etc/compatconfig"));
        Files.createDirectories(apex.resolve("active/etc/compatconfig"));
        Files.createDirectories(apex.resolve("inactive/etc/compatconfig"));
        Files.createDirectories(apex.resolve("empty"));
        Files.write(apex.resolve("active/etc/compatconfig/policy.xml"), bytes("<config/>"));
        Files.write(apex.resolve("inactive/etc/compatconfig/ignored.xml"), bytes("<config/>"));
        String valid = module("active", "true") + module("inactive", "false") + module("empty", "true");
        inventory(apex, valid);
        List<File> selected = SystemCompatCatalog.files(system.toFile(), apex.toFile());
        if (selected.size() != 1 || !selected.get(0).getName().equals("policy.xml")) {
            throw new AssertionError("Active APEX selection is incorrect");
        }
        inventory(apex, module("active", "true") + module("active", "true"));
        reject(() -> SystemCompatCatalog.files(system.toFile(), apex.toFile()));
        inventory(apex, module("../active", "true"));
        reject(() -> SystemCompatCatalog.files(system.toFile(), apex.toFile()));
        inventory(apex, module("missing", "true"));
        reject(() -> SystemCompatCatalog.files(system.toFile(), apex.toFile()));
        inventory(apex, module("active", "yes"));
        reject(() -> SystemCompatCatalog.files(system.toFile(), apex.toFile()));
        Files.write(apex.resolve("apex-info-list.xml"), bytes("<apex-info-list><unknown>"));
        reject(() -> SystemCompatCatalog.files(system.toFile(), apex.toFile()));
        inventory(apex, valid);
        Files.createSymbolicLink(apex.resolve("active/etc/compatconfig/link.xml"), new File("policy.xml").toPath());
        reject(() -> SystemCompatCatalog.files(system.toFile(), apex.toFile()));
        System.out.println("system compat input: complete XML framing and strict active APEX topology PASS (not ART policy evaluation)");
    }
}
