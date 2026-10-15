package com.android.server.pm;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.Arrays;

/** Trace the original libcore comparator calls without replacing the comparator/sort. */
public final class TimSortOracle {
    public static void main(String[] args) throws Exception {
        File[] files = new File(args[0]).listFiles((dir, name) -> name.endsWith(".input"));
        Arrays.sort(files, (a, b) -> a.getName().compareTo(b.getName()));
        for (File file : files) {
            var lines = Files.readAllLines(file.toPath(), StandardCharsets.UTF_8);
            int[] values = new int[lines.size()];
            Integer[] order = new Integer[lines.size()];
            for (int i = 0; i < values.length; i++) {
                values[i] = Integer.parseInt(lines.get(i));
                order[i] = i;
            }
            StringBuilder trace = new StringBuilder();
            Arrays.sort(order, (a, b) -> {
                trace.append(a).append(' ').append(b).append('\n');
                return Integer.compare(values[a], values[b]);
            });
            StringBuilder sorted = new StringBuilder();
            for (int index : order) sorted.append(index).append('\n');
            Files.write(new File(file.getPath() + ".trace").toPath(), trace.toString().getBytes(StandardCharsets.UTF_8));
            Files.write(new File(file.getPath() + ".sorted").toPath(), sorted.toString().getBytes(StandardCharsets.UTF_8));
        }
        System.out.println("TIMSORT " + files.length);
    }
}
