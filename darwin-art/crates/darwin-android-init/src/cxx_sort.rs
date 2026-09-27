//! libc++'s `std::sort` for non-arithmetic element types (LLVM 19-21
//! `__introsort` with `__partition_with_equals_on_right/left`, heap-sort
//! fallback), ported so that unstable orderings match the C++ toolchain.
//!
//! libpropertyinfoserializer sorts prefix entries by descending length with
//! `std::sort`; entries of equal length keep whatever order libc++ leaves
//! them in, and that order is visible in the serialized bytes.

pub(crate) fn sort_by<T: Clone>(v: &mut [T], mut less: impl FnMut(&T, &T) -> bool) {
    let len = v.len();
    if len == 0 {
        return;
    }
    // __bit_log2
    let depth = 2 * (usize::BITS - 1 - len.leading_zeros()) as isize;
    introsort(v, 0, len, &mut less, depth, true);
}

fn sort3<T>(v: &mut [T], x: usize, y: usize, z: usize, c: &mut impl FnMut(&T, &T) -> bool) {
    if !c(&v[y], &v[x]) {
        if !c(&v[z], &v[y]) {
            return;
        }
        v.swap(y, z);
        if c(&v[y], &v[x]) {
            v.swap(x, y);
        }
        return;
    }
    if c(&v[z], &v[y]) {
        v.swap(x, z);
        return;
    }
    v.swap(x, y);
    if c(&v[z], &v[y]) {
        v.swap(y, z);
    }
}

fn sort4<T>(
    v: &mut [T],
    x1: usize,
    x2: usize,
    x3: usize,
    x4: usize,
    c: &mut impl FnMut(&T, &T) -> bool,
) {
    sort3(v, x1, x2, x3, c);
    if c(&v[x4], &v[x3]) {
        v.swap(x3, x4);
        if c(&v[x3], &v[x2]) {
            v.swap(x2, x3);
            if c(&v[x2], &v[x1]) {
                v.swap(x1, x2);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn sort5<T>(
    v: &mut [T],
    x1: usize,
    x2: usize,
    x3: usize,
    x4: usize,
    x5: usize,
    c: &mut impl FnMut(&T, &T) -> bool,
) {
    sort4(v, x1, x2, x3, x4, c);
    if c(&v[x5], &v[x4]) {
        v.swap(x4, x5);
        if c(&v[x4], &v[x3]) {
            v.swap(x3, x4);
            if c(&v[x3], &v[x2]) {
                v.swap(x2, x3);
                if c(&v[x2], &v[x1]) {
                    v.swap(x1, x2);
                }
            }
        }
    }
}

fn insertion_sort<T: Clone>(
    v: &mut [T],
    first: usize,
    last: usize,
    c: &mut impl FnMut(&T, &T) -> bool,
) {
    if first == last {
        return;
    }
    let mut i = first + 1;
    while i != last {
        let mut j = i - 1;
        if c(&v[i], &v[j]) {
            let t = v[i].clone();
            let mut k = j;
            j = i;
            loop {
                v[j] = v[k].clone();
                j = k;
                if j == first {
                    break;
                }
                k -= 1;
                if !c(&t, &v[k]) {
                    break;
                }
            }
            v[j] = t;
        }
        i += 1;
    }
}

fn insertion_sort_unguarded<T: Clone>(
    v: &mut [T],
    first: usize,
    last: usize,
    c: &mut impl FnMut(&T, &T) -> bool,
) {
    if first == last {
        return;
    }
    let mut i = first + 1;
    while i != last {
        let mut j = i - 1;
        if c(&v[i], &v[j]) {
            let t = v[i].clone();
            let mut k = j;
            j = i;
            loop {
                v[j] = v[k].clone();
                j = k;
                k -= 1;
                if !c(&t, &v[k]) {
                    break;
                }
            }
            v[j] = t;
        }
        i += 1;
    }
}

fn insertion_sort_incomplete<T: Clone>(
    v: &mut [T],
    first: usize,
    last: usize,
    c: &mut impl FnMut(&T, &T) -> bool,
) -> bool {
    match last - first {
        0 | 1 => return true,
        2 => {
            if c(&v[last - 1], &v[first]) {
                v.swap(first, last - 1);
            }
            return true;
        }
        3 => {
            sort3(v, first, first + 1, last - 1, c);
            return true;
        }
        4 => {
            sort4(v, first, first + 1, first + 2, last - 1, c);
            return true;
        }
        5 => {
            sort5(v, first, first + 1, first + 2, first + 3, last - 1, c);
            return true;
        }
        _ => {}
    }
    let mut j = first + 2;
    sort3(v, first, first + 1, j, c);
    const LIMIT: u32 = 8;
    let mut count = 0;
    let mut i = j + 1;
    while i != last {
        if c(&v[i], &v[j]) {
            let t = v[i].clone();
            let mut k = j;
            j = i;
            loop {
                v[j] = v[k].clone();
                j = k;
                if j == first {
                    break;
                }
                k -= 1;
                if !c(&t, &v[k]) {
                    break;
                }
            }
            v[j] = t;
            count += 1;
            if count == LIMIT {
                return i + 1 == last;
            }
        }
        j = i;
        i += 1;
    }
    true
}

fn partition_with_equals_on_right<T: Clone>(
    v: &mut [T],
    begin: usize,
    end: usize,
    c: &mut impl FnMut(&T, &T) -> bool,
) -> (usize, bool) {
    let pivot = v[begin].clone();
    let mut first = begin;
    let mut last = end;
    loop {
        first += 1;
        if !c(&v[first], &pivot) {
            break;
        }
    }
    if begin == first - 1 {
        while first < last {
            last -= 1;
            if c(&v[last], &pivot) {
                break;
            }
        }
    } else {
        loop {
            last -= 1;
            if c(&v[last], &pivot) {
                break;
            }
        }
    }
    let already_partitioned = first >= last;
    while first < last {
        v.swap(first, last);
        loop {
            first += 1;
            if !c(&v[first], &pivot) {
                break;
            }
        }
        loop {
            last -= 1;
            if c(&v[last], &pivot) {
                break;
            }
        }
    }
    let pivot_pos = first - 1;
    if begin != pivot_pos {
        v[begin] = v[pivot_pos].clone();
    }
    v[pivot_pos] = pivot;
    (pivot_pos, already_partitioned)
}

fn partition_with_equals_on_left<T: Clone>(
    v: &mut [T],
    begin: usize,
    end: usize,
    c: &mut impl FnMut(&T, &T) -> bool,
) -> usize {
    let pivot = v[begin].clone();
    let mut first = begin;
    let mut last = end;
    if c(&pivot, &v[last - 1]) {
        loop {
            first += 1;
            if c(&pivot, &v[first]) {
                break;
            }
        }
    } else {
        loop {
            first += 1;
            // while (++first < last && !comp(pivot, *first))
            if first >= last || c(&pivot, &v[first]) {
                break;
            }
        }
    }
    if first < last {
        loop {
            last -= 1;
            if !c(&pivot, &v[last]) {
                break;
            }
        }
    }
    while first < last {
        v.swap(first, last);
        loop {
            first += 1;
            if c(&pivot, &v[first]) {
                break;
            }
        }
        loop {
            last -= 1;
            if !c(&pivot, &v[last]) {
                break;
            }
        }
    }
    let pivot_pos = first - 1;
    if begin != pivot_pos {
        v[begin] = v[pivot_pos].clone();
    }
    v[pivot_pos] = pivot;
    first
}

fn sift_down<T: Clone>(
    v: &mut [T],
    first: usize,
    c: &mut impl FnMut(&T, &T) -> bool,
    len: isize,
    start: usize,
) {
    let mut child = (start - first) as isize;
    if len < 2 || (len - 2) / 2 < child {
        return;
    }
    child = 2 * child + 1;
    let mut child_i = first + child as usize;
    if child + 1 < len && c(&v[child_i], &v[child_i + 1]) {
        child_i += 1;
        child += 1;
    }
    if c(&v[child_i], &v[start]) {
        return;
    }
    let top = v[start].clone();
    let mut start = start;
    loop {
        v[start] = v[child_i].clone();
        start = child_i;
        if (len - 2) / 2 < child {
            break;
        }
        child = 2 * child + 1;
        child_i = first + child as usize;
        if child + 1 < len && c(&v[child_i], &v[child_i + 1]) {
            child_i += 1;
            child += 1;
        }
        if c(&v[child_i], &top) {
            break;
        }
    }
    v[start] = top;
}

fn floyd_sift_down<T: Clone>(
    v: &mut [T],
    first: usize,
    c: &mut impl FnMut(&T, &T) -> bool,
    len: isize,
) -> usize {
    let mut hole = first;
    let mut child_i = first;
    let mut child: isize = 0;
    loop {
        child_i += (child + 1) as usize;
        child = 2 * child + 1;
        if child + 1 < len && c(&v[child_i], &v[child_i + 1]) {
            child_i += 1;
            child += 1;
        }
        v[hole] = v[child_i].clone();
        hole = child_i;
        if child > (len - 2) / 2 {
            return hole;
        }
    }
}

fn sift_up<T: Clone>(
    v: &mut [T],
    first: usize,
    last: usize,
    c: &mut impl FnMut(&T, &T) -> bool,
    len: isize,
) {
    if len > 1 {
        let mut len = (len - 2) / 2;
        let mut ptr = first + len as usize;
        let mut last = last - 1;
        if c(&v[ptr], &v[last]) {
            let t = v[last].clone();
            loop {
                v[last] = v[ptr].clone();
                last = ptr;
                if len == 0 {
                    break;
                }
                len = (len - 1) / 2;
                ptr = first + len as usize;
                if !c(&v[ptr], &t) {
                    break;
                }
            }
            v[last] = t;
        }
    }
}

fn heap_sort<T: Clone>(v: &mut [T], first: usize, last: usize, c: &mut impl FnMut(&T, &T) -> bool) {
    let n = (last - first) as isize;
    if n > 1 {
        let mut start = (n - 2) / 2;
        while start >= 0 {
            sift_down(v, first, c, n, first + start as usize);
            start -= 1;
        }
    }
    let mut end = last;
    let mut n = n;
    while n > 1 {
        let top = v[first].clone();
        let hole = floyd_sift_down(v, first, c, n);
        let last_elem = end - 1;
        if hole == last_elem {
            v[hole] = top;
        } else {
            v[hole] = v[last_elem].clone();
            let hole_end = hole + 1;
            v[last_elem] = top;
            sift_up(v, first, hole_end, c, (hole_end - first) as isize);
        }
        end -= 1;
        n -= 1;
    }
}

fn introsort<T: Clone>(
    v: &mut [T],
    mut first: usize,
    mut last: usize,
    c: &mut impl FnMut(&T, &T) -> bool,
    mut depth: isize,
    mut leftmost: bool,
) {
    const LIMIT: usize = 24;
    const NINTHER_THRESHOLD: usize = 128;
    loop {
        let len = last - first;
        match len {
            0 | 1 => return,
            2 => {
                if c(&v[last - 1], &v[first]) {
                    v.swap(first, last - 1);
                }
                return;
            }
            3 => {
                sort3(v, first, first + 1, last - 1, c);
                return;
            }
            4 => {
                sort4(v, first, first + 1, first + 2, last - 1, c);
                return;
            }
            5 => {
                sort5(v, first, first + 1, first + 2, first + 3, last - 1, c);
                return;
            }
            _ => {}
        }
        if len < LIMIT {
            if leftmost {
                insertion_sort(v, first, last, c);
            } else {
                insertion_sort_unguarded(v, first, last, c);
            }
            return;
        }
        if depth == 0 {
            heap_sort(v, first, last, c);
            return;
        }
        depth -= 1;
        let half = len / 2;
        if len > NINTHER_THRESHOLD {
            sort3(v, first, first + half, last - 1, c);
            sort3(v, first + 1, first + (half - 1), last - 2, c);
            sort3(v, first + 2, first + (half + 1), last - 3, c);
            sort3(v, first + (half - 1), first + half, first + (half + 1), c);
            v.swap(first, first + half);
        } else {
            sort3(v, first + half, first, last - 1, c);
        }
        if !leftmost && !c(&v[first - 1], &v[first]) {
            first = partition_with_equals_on_left(v, first, last, c);
            continue;
        }
        let (mut i, already_partitioned) = partition_with_equals_on_right(v, first, last, c);
        if already_partitioned {
            let fs = insertion_sort_incomplete(v, first, i, c);
            if insertion_sort_incomplete(v, i + 1, last, c) {
                if fs {
                    return;
                }
                last = i;
                continue;
            } else if fs {
                i += 1;
                first = i;
                continue;
            }
        }
        introsort(v, first, i, c, depth, leftmost);
        leftmost = false;
        i += 1;
        first = i;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_and_is_deterministic() {
        let mut state = 12345u64;
        for len in 0..400 {
            let mut v: Vec<(u32, u32)> = (0..len)
                .map(|i| {
                    state = state
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    (((state >> 33) % 7) as u32, i as u32)
                })
                .collect();
            sort_by(&mut v, |a, b| a.0 > b.0);
            assert!(v.windows(2).all(|w| w[0].0 >= w[1].0));
        }
    }
}
