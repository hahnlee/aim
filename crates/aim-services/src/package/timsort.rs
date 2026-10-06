// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0
/*
 * Copyright (c) 2009, 2013, Oracle and/or its affiliates. All rights reserved.
 * Copyright 2009 Google Inc.  All Rights Reserved.
 * DO NOT ALTER OR REMOVE COPYRIGHT NOTICES OR THIS FILE HEADER.
 *
 * This code is free software; you can redistribute it and/or modify it
 * under the terms of the GNU General Public License version 2 only, as
 * published by the Free Software Foundation.  Oracle designates this
 * particular file as subject to the "Classpath" exception as provided
 * by Oracle in the LICENSE file that accompanied this code.
 *
 * This code is distributed in the hope that it will be useful, but WITHOUT
 * ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or
 * FITNESS FOR A PARTICULAR PURPOSE.  See the GNU General Public License
 * version 2 for more details (a copy is included in the LICENSE file that
 * accompanied this code).
 *
 * You should have received a copy of the GNU General Public License version
 * 2 along with this work; if not, write to the Free Software Foundation,
 * Inc., 51 Franklin St, Fifth Floor, Boston, MA 02110-1301 USA.
 *
 * Please contact Oracle, 500 Oracle Parkway, Redwood Shores, CA 94065 USA
 * or visit www.oracle.com if you need additional information or have any
 * questions.
 */
//! Rust port of libcore java.util.TimSort, platform/libcore
//! fff4fcc0cf7f080cf2511cbb57561482b13b218f (android-16.0.0_r1),
//! ojluni/src/main/java/java/util/TimSort.java. Uses indices, Rust vectors
//! and Result diagnostics, preserving the original comparator sequence.
//! License/exception text: licensing/third-party/libcore-ojluni-LICENSE.
//! Index sorting keeps value ownership separate from comparator diagnostics.
use std::cmp::Ordering::{self, Greater, Less};
type Result<T> = std::result::Result<T, String>;

pub fn sort(n: usize, cmp: impl FnMut(usize, usize) -> Result<Ordering>) -> Result<Vec<usize>> {
    let mut sort = Sort {
        a: (0..n).collect(),
        runs: vec![],
        min_gallop: 7,
        cmp,
    };
    if n < 2 {
        return Ok(sort.a);
    }
    if n < 32 {
        let run = sort.run(0, n)?;
        sort.binary(0, n, run)?;
        return Ok(sort.a);
    }
    let (mut min_run, mut remainder) = (n, 0);
    while min_run >= 32 {
        remainder |= min_run & 1;
        min_run >>= 1;
    }
    min_run += remainder;
    let mut base = 0;
    while base < n {
        let mut run = sort.run(base, n)?;
        if run < min_run {
            let force = min_run.min(n - base);
            sort.binary(base, base + force, base + run)?;
            run = force;
        }
        sort.runs.push((base, run));
        sort.collapse(false)?;
        base += run;
    }
    sort.collapse(true)?;
    Ok(sort.a)
}

struct Sort<C> {
    a: Vec<usize>,
    runs: Vec<(usize, usize)>,
    min_gallop: i32,
    cmp: C,
}
impl<C: FnMut(usize, usize) -> Result<Ordering>> Sort<C> {
    fn run(&mut self, lo: usize, hi: usize) -> Result<usize> {
        if lo + 1 == hi {
            return Ok(1);
        }
        let descending = (self.cmp)(self.a[lo + 1], self.a[lo])? == Less;
        let mut end = lo + 2;
        while end < hi {
            let order = (self.cmp)(self.a[end], self.a[end - 1])?;
            if (descending && order != Less) || (!descending && order == Less) {
                break;
            }
            end += 1;
        }
        if descending {
            self.a[lo..end].reverse();
        }
        Ok(end - lo)
    }
    fn binary(&mut self, lo: usize, hi: usize, start: usize) -> Result<()> {
        for at in start..hi {
            let pivot = self.a[at];
            let (mut left, mut right) = (lo, at);
            while left < right {
                let middle = (left + right) / 2;
                if (self.cmp)(pivot, self.a[middle])? == Less {
                    right = middle;
                } else {
                    left = middle + 1;
                }
            }
            self.a.copy_within(left..at, left + 1);
            self.a[left] = pivot;
        }
        Ok(())
    }
    fn collapse(&mut self, force: bool) -> Result<()> {
        while self.runs.len() > 1 {
            let mut n = self.runs.len() - 2;
            let len = |i: usize| self.runs[i].1;
            if force {
                if n > 0 && len(n - 1) < len(n + 1) {
                    n -= 1;
                }
            } else if (n > 0 && len(n - 1) <= len(n) + len(n + 1))
                || (n > 1 && len(n - 2) <= len(n) + len(n - 1))
            {
                if len(n - 1) < len(n + 1) {
                    n -= 1;
                }
            } else if len(n) > len(n + 1) {
                break;
            }
            self.merge(n)?;
        }
        Ok(())
    }
    fn merge(&mut self, at: usize) -> Result<()> {
        let (mut base1, mut len1) = self.runs[at];
        let (base2, mut len2) = self.runs[at + 1];
        self.runs[at].1 = len1 + len2;
        self.runs.remove(at + 1);
        let skip = gallop(
            self.a[base2],
            &self.a[base1..base1 + len1],
            0,
            true,
            &mut self.cmp,
        )?;
        base1 += skip;
        len1 -= skip;
        if len1 == 0 {
            return Ok(());
        }
        len2 = gallop(
            self.a[base1 + len1 - 1],
            &self.a[base2..base2 + len2],
            len2 - 1,
            false,
            &mut self.cmp,
        )?;
        if len2 == 0 {
            return Ok(());
        }
        if len1 <= len2 {
            self.low(base1, len1, base2, len2)
        } else {
            self.high(base1, len1, base2, len2)
        }
    }
    fn low(&mut self, base1: usize, mut len1: usize, base2: usize, mut len2: usize) -> Result<()> {
        let tmp = self.a[base1..base1 + len1].to_vec();
        let (mut c1, mut c2, mut dest) = (0, base2, base1);
        self.a[dest] = self.a[c2];
        dest += 1;
        c2 += 1;
        len2 -= 1;
        if len2 == 0 {
            self.a[dest..dest + len1].copy_from_slice(&tmp[c1..c1 + len1]);
            return Ok(());
        }
        if len1 == 1 {
            self.a.copy_within(c2..c2 + len2, dest);
            self.a[dest + len2] = tmp[c1];
            return Ok(());
        }
        let mut min = self.min_gallop;
        'outer: loop {
            let (mut count1, mut count2) = (0, 0);
            loop {
                if (self.cmp)(self.a[c2], tmp[c1])? == Less {
                    self.a[dest] = self.a[c2];
                    dest += 1;
                    c2 += 1;
                    count2 += 1;
                    count1 = 0;
                    len2 -= 1;
                    if len2 == 0 {
                        break 'outer;
                    }
                } else {
                    self.a[dest] = tmp[c1];
                    dest += 1;
                    c1 += 1;
                    count1 += 1;
                    count2 = 0;
                    len1 -= 1;
                    if len1 == 1 {
                        break 'outer;
                    }
                }
                if (count1 | count2) >= min {
                    break;
                }
            }
            loop {
                let count1 = gallop(self.a[c2], &tmp[c1..c1 + len1], 0, true, &mut self.cmp)?;
                if count1 != 0 {
                    self.a[dest..dest + count1].copy_from_slice(&tmp[c1..c1 + count1]);
                    dest += count1;
                    c1 += count1;
                    len1 -= count1;
                    if len1 <= 1 {
                        break 'outer;
                    }
                }
                self.a[dest] = self.a[c2];
                dest += 1;
                c2 += 1;
                len2 -= 1;
                if len2 == 0 {
                    break 'outer;
                }
                let count2 = gallop(tmp[c1], &self.a[c2..c2 + len2], 0, false, &mut self.cmp)?;
                if count2 != 0 {
                    self.a.copy_within(c2..c2 + count2, dest);
                    dest += count2;
                    c2 += count2;
                    len2 -= count2;
                    if len2 == 0 {
                        break 'outer;
                    }
                }
                self.a[dest] = tmp[c1];
                dest += 1;
                c1 += 1;
                len1 -= 1;
                if len1 == 1 {
                    break 'outer;
                }
                min -= 1;
                if count1 < 7 && count2 < 7 {
                    break;
                }
            }
            min = min.max(0) + 2;
        }
        self.min_gallop = min.max(1);
        if len1 == 1 {
            self.a.copy_within(c2..c2 + len2, dest);
            self.a[dest + len2] = tmp[c1];
        } else if len1 == 0 {
            return Err("Comparison method violates its general contract!".into());
        } else {
            self.a[dest..dest + len1].copy_from_slice(&tmp[c1..c1 + len1]);
        }
        Ok(())
    }
    fn high(&mut self, base1: usize, mut len1: usize, base2: usize, mut len2: usize) -> Result<()> {
        let tmp = self.a[base2..base2 + len2].to_vec();
        // Exclusive cursors avoid a negative index after the last consumed item.
        let (mut c1, mut c2, mut dest) = (base1 + len1, len2, base2 + len2);
        c1 -= 1;
        dest -= 1;
        self.a[dest] = self.a[c1];
        len1 -= 1;
        if len1 == 0 {
            self.a[dest - len2..dest].copy_from_slice(&tmp[..len2]);
            return Ok(());
        }
        if len2 == 1 {
            dest -= len1;
            self.a.copy_within(c1 - len1..c1, dest);
            self.a[dest - 1] = tmp[c2 - 1];
            return Ok(());
        }
        let mut min = self.min_gallop;
        'outer: loop {
            let (mut count1, mut count2) = (0, 0);
            loop {
                if (self.cmp)(tmp[c2 - 1], self.a[c1 - 1])? == Less {
                    dest -= 1;
                    c1 -= 1;
                    self.a[dest] = self.a[c1];
                    count1 += 1;
                    count2 = 0;
                    len1 -= 1;
                    if len1 == 0 {
                        break 'outer;
                    }
                } else {
                    dest -= 1;
                    c2 -= 1;
                    self.a[dest] = tmp[c2];
                    count2 += 1;
                    count1 = 0;
                    len2 -= 1;
                    if len2 == 1 {
                        break 'outer;
                    }
                }
                if (count1 | count2) >= min {
                    break;
                }
            }
            loop {
                let count1 = len1
                    - gallop(
                        tmp[c2 - 1],
                        &self.a[base1..base1 + len1],
                        len1 - 1,
                        true,
                        &mut self.cmp,
                    )?;
                if count1 != 0 {
                    dest -= count1;
                    c1 -= count1;
                    len1 -= count1;
                    self.a.copy_within(c1..c1 + count1, dest);
                    if len1 == 0 {
                        break 'outer;
                    }
                }
                dest -= 1;
                c2 -= 1;
                self.a[dest] = tmp[c2];
                len2 -= 1;
                if len2 == 1 {
                    break 'outer;
                }
                let count2 =
                    len2 - gallop(self.a[c1 - 1], &tmp[..len2], len2 - 1, false, &mut self.cmp)?;
                if count2 != 0 {
                    dest -= count2;
                    c2 -= count2;
                    len2 -= count2;
                    self.a[dest..dest + count2].copy_from_slice(&tmp[c2..c2 + count2]);
                    if len2 <= 1 {
                        break 'outer;
                    }
                }
                dest -= 1;
                c1 -= 1;
                self.a[dest] = self.a[c1];
                len1 -= 1;
                if len1 == 0 {
                    break 'outer;
                }
                min -= 1;
                if count1 < 7 && count2 < 7 {
                    break;
                }
            }
            min = min.max(0) + 2;
        }
        self.min_gallop = min.max(1);
        if len2 == 1 {
            dest -= len1;
            self.a.copy_within(c1 - len1..c1, dest);
            self.a[dest - 1] = tmp[c2 - 1];
        } else if len2 == 0 {
            return Err("Comparison method violates its general contract!".into());
        } else {
            self.a[dest - len2..dest].copy_from_slice(&tmp[..len2]);
        }
        Ok(())
    }
}

fn gallop(
    key: usize,
    a: &[usize],
    hint: usize,
    right: bool,
    cmp: &mut impl FnMut(usize, usize) -> Result<Ordering>,
) -> Result<usize> {
    let advance = |order| {
        if right {
            order != Less
        } else {
            order == Greater
        }
    };
    let (mut last, mut offset) = (0isize, 1isize);
    if advance(cmp(key, a[hint])?) {
        let max = (a.len() - hint) as isize;
        while offset < max && advance(cmp(key, a[hint + offset as usize])?) {
            last = offset;
            offset = offset.saturating_mul(2).saturating_add(1);
        }
        offset = offset.min(max);
        last += hint as isize;
        offset += hint as isize;
    } else {
        let max = (hint + 1) as isize;
        while offset < max && !advance(cmp(key, a[hint - offset as usize])?) {
            last = offset;
            offset = offset.saturating_mul(2).saturating_add(1);
        }
        offset = offset.min(max);
        let old = last;
        last = hint as isize - offset;
        offset = hint as isize - old;
    }
    last += 1;
    while last < offset {
        let middle = last + (offset - last) / 2;
        if advance(cmp(key, a[middle as usize])?) {
            last = middle + 1;
        } else {
            offset = middle;
        }
    }
    Ok(offset as usize)
}

#[cfg(test)]
#[path = "../../tests/common/java.rs"]
mod java;
#[cfg(test)]
#[path = "../../tests/common/runtime.rs"]
mod runtime;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires pinned image, aimctl, JDK and d8; run explicitly"]
    fn comparisons_and_sorted_order_match_original_libcore() {
        use super::runtime::{Boot, Data, run};
        use std::{
            fs,
            process::Command,
            time::{Duration, Instant},
        };
        let directory = std::env::temp_dir().join(format!("aim-timsort-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let data = Data(directory);
        let java = aim_paths::fetched().join("java");
        let jdk = java.join("temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
        let classes = data.0.join("classes");
        let dex = data.0.join("dex");
        fs::create_dir(&classes).unwrap();
        fs::create_dir(&dex).unwrap();
        run(Command::new(jdk.join("bin/javac"))
            .args(["--release", "17", "-d"])
            .arg(&classes)
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/TimSortOracle.java"),
            ));
        run(Command::new(jdk.join("bin/java"))
            .arg("-cp")
            .arg(java.join("build-tools-36.0.0/android-16/lib/d8.jar"))
            .args([
                "com.android.tools.r8.D8",
                "--release",
                "--min-api",
                "36",
                "--lib",
            ])
            .arg(&jdk)
            .arg("--output")
            .arg(&dex)
            .arg(classes.join("com/android/server/pm/TimSortOracle.class")));
        super::java::check_linkage(&dex.join("classes.dex"), &[]).unwrap();
        let boot = Boot {
            ctl: aim_paths::root().join("target/release/aimctl"),
            data: data.0.join("guest"),
        };
        run(boot.command().args(["start", "--windows"]));
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let output = boot
                .command()
                .args(["shell", "getprop", "sys.boot_completed"])
                .output()
                .unwrap();
            if output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "1" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "disposable boot did not complete"
            );
            std::thread::sleep(Duration::from_secs(1));
        }
        let input = boot.data.join("data/local/tmp/timsort");
        fs::create_dir(&input).unwrap();
        fs::copy(dex.join("classes.dex"), input.join("oracle.dex")).unwrap();
        let mut random = 0x1836u64;
        let mut expected = Vec::new();
        for n in [
            0, 1, 2, 7, 31, 32, 33, 63, 64, 65, 127, 128, 129, 257, 1024, 4096,
        ] {
            for kinds in [2, 17] {
                for _ in 0..4 {
                    let values: Vec<_> = (0..n)
                        .map(|_| {
                            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                            (random >> 32) % kinds
                        })
                        .collect();
                    let mut trace = String::new();
                    let indices = sort(n, |a, b| {
                        trace.push_str(&format!("{a} {b}\n"));
                        Ok(values[a].cmp(&values[b]))
                    })
                    .unwrap();
                    let filename = format!("{}.input", expected.len());
                    let text: String = values.iter().map(|v| format!("{v}\n")).collect();
                    fs::write(input.join(&filename), text).unwrap();
                    let sorted: String = indices.into_iter().map(|i| format!("{i}\n")).collect();
                    expected.push((filename, trace, sorted));
                }
            }
        }
        let result = run(boot.command().args([
            "shell",
            "/system/bin/app_process",
            "-Djava.class.path=/data/local/tmp/timsort/oracle.dex",
            "/system/bin",
            "com.android.server.pm.TimSortOracle",
            "/data/local/tmp/timsort",
        ]));
        assert_eq!(
            String::from_utf8(result.stdout).unwrap(),
            format!("TIMSORT {}\n", expected.len())
        );
        for (file, trace, sorted) in expected {
            assert_eq!(
                fs::read_to_string(input.join(format!("{file}.sorted"))).unwrap(),
                sorted,
                "sorted order: {file}"
            );
            let actual = fs::read_to_string(input.join(format!("{file}.trace"))).unwrap();
            if actual != trace {
                let mismatch = actual.lines().zip(trace.lines()).position(|(a, b)| a != b);
                panic!(
                    "comparison order: {file}, first differing pair {mismatch:?}, original {} pairs, native {} pairs",
                    actual.lines().count(),
                    trace.lines().count()
                );
            }
        }
    }

    #[test]
    fn stable_sort_handles_runs_gallops_and_merge_directions() {
        let mut random = 0x1836u64;
        for n in [0, 1, 2, 7, 31, 32, 33, 63, 64, 65, 127, 128, 257, 1024] {
            for kinds in [2, 17] {
                for _ in 0..30 {
                    let values: Vec<_> = (0..n)
                        .map(|_| {
                            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                            (random >> 32) % kinds
                        })
                        .collect();
                    let actual = sort(n, |a, b| Ok(values[a].cmp(&values[b]))).unwrap();
                    let mut expected: Vec<_> = (0..n).collect();
                    expected.sort_by_key(|i| values[*i]);
                    assert_eq!(actual, expected, "n={n} kinds={kinds}");
                }
            }
        }
    }
}
