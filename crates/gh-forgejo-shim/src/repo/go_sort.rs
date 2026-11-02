//! Go's `sort.Sort`, ported line for line from Go 1.25's `sort/zsortinterface.go` and
//! `sort/sort.go`.
//!
//! gh orders a checkout's remotes with it (`gh/context/remote.go`, `sort.Sort(resolvedRemotes)`).
//! The sort isn't stable, so when more than 12 remotes share a rank, which one comes first depends
//! on this exact algorithm. It is deterministic: its only randomness is an xorshift seeded with the
//! slice length.

/// Sorts `data` so that `less` holds between neighbours, ordering equal elements as Go's
/// `sort.Sort` does.
pub(crate) fn sort<T>(data: &mut [T], less: impl Fn(&T, &T) -> bool) {
    let n = data.len();
    if n <= 1 {
        return;
    }
    let limit = bits_len(n);
    let mut sorter = Sorter { data, less };
    sorter.pdqsort(0, len(n), limit);
}

/// Go's `bits.Len`: the number of bits needed to represent `x`.
fn bits_len(x: usize) -> u32 {
    usize::BITS - x.leading_zeros()
}

fn len(n: usize) -> isize {
    isize::try_from(n).unwrap_or(isize::MAX)
}

fn index(i: isize) -> usize {
    usize::try_from(i).unwrap_or_default()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hint {
    Unknown,
    Increasing,
    Decreasing,
}

struct Sorter<'a, T, F> {
    data: &'a mut [T],
    less: F,
}

impl<T, F: Fn(&T, &T) -> bool> Sorter<'_, T, F> {
    fn less(&self, i: isize, j: isize) -> bool {
        (self.less)(&self.data[index(i)], &self.data[index(j)])
    }

    fn swap(&mut self, i: isize, j: isize) {
        self.data.swap(index(i), index(j));
    }

    fn insertion_sort(&mut self, a: isize, b: isize) {
        let mut i = a + 1;
        while i < b {
            let mut j = i;
            while j > a && self.less(j, j - 1) {
                self.swap(j, j - 1);
                j -= 1;
            }
            i += 1;
        }
    }

    fn sift_down(&mut self, lo: isize, hi: isize, first: isize) {
        let mut root = lo;
        loop {
            let mut child = 2 * root + 1;
            if child >= hi {
                break;
            }
            if child + 1 < hi && self.less(first + child, first + child + 1) {
                child += 1;
            }
            if !self.less(first + root, first + child) {
                return;
            }
            self.swap(first + root, first + child);
            root = child;
        }
    }

    fn heap_sort(&mut self, a: isize, b: isize) {
        let first = a;
        let lo = 0;
        let hi = b - a;
        let mut i = (hi - 1) / 2;
        while i >= 0 {
            self.sift_down(i, hi, first);
            i -= 1;
        }
        let mut i = hi - 1;
        while i >= 0 {
            self.swap(first, first + i);
            self.sift_down(lo, i, first);
            i -= 1;
        }
    }

    fn pdqsort(&mut self, mut a: isize, mut b: isize, mut limit: u32) {
        const MAX_INSERTION: isize = 12;
        let mut was_balanced = true;
        let mut was_partitioned = true;
        loop {
            let length = b - a;
            if length <= MAX_INSERTION {
                self.insertion_sort(a, b);
                return;
            }
            if limit == 0 {
                self.heap_sort(a, b);
                return;
            }
            if !was_balanced {
                self.break_patterns(a, b);
                limit -= 1;
            }

            let (mut pivot, mut hint) = self.choose_pivot(a, b);
            if hint == Hint::Decreasing {
                self.reverse_range(a, b);
                pivot = (b - 1) - (pivot - a);
                hint = Hint::Increasing;
            }

            if was_balanced
                && was_partitioned
                && hint == Hint::Increasing
                && self.partial_insertion_sort(a, b)
            {
                return;
            }

            if a > 0 && !self.less(a - 1, pivot) {
                a = self.partition_equal(a, b, pivot);
                continue;
            }

            let (mid, already_partitioned) = self.partition(a, b, pivot);
            was_partitioned = already_partitioned;

            let (left_len, right_len) = (mid - a, b - mid);
            let balance_threshold = length / 8;
            if left_len < right_len {
                was_balanced = left_len >= balance_threshold;
                self.pdqsort(a, mid, limit);
                a = mid + 1;
            } else {
                was_balanced = right_len >= balance_threshold;
                self.pdqsort(mid + 1, b, limit);
                b = mid;
            }
        }
    }

    fn partition(&mut self, a: isize, b: isize, pivot: isize) -> (isize, bool) {
        self.swap(a, pivot);
        let (mut i, mut j) = (a + 1, b - 1);
        while i <= j && self.less(i, a) {
            i += 1;
        }
        while i <= j && !self.less(j, a) {
            j -= 1;
        }
        if i > j {
            self.swap(j, a);
            return (j, true);
        }
        self.swap(i, j);
        i += 1;
        j -= 1;
        loop {
            while i <= j && self.less(i, a) {
                i += 1;
            }
            while i <= j && !self.less(j, a) {
                j -= 1;
            }
            if i > j {
                break;
            }
            self.swap(i, j);
            i += 1;
            j -= 1;
        }
        self.swap(j, a);
        (j, false)
    }

    fn partition_equal(&mut self, a: isize, b: isize, pivot: isize) -> isize {
        self.swap(a, pivot);
        let (mut i, mut j) = (a + 1, b - 1);
        loop {
            while i <= j && !self.less(a, i) {
                i += 1;
            }
            while i <= j && self.less(a, j) {
                j -= 1;
            }
            if i > j {
                break;
            }
            self.swap(i, j);
            i += 1;
            j -= 1;
        }
        i
    }

    fn partial_insertion_sort(&mut self, a: isize, b: isize) -> bool {
        const MAX_STEPS: usize = 5;
        const SHORTEST_SHIFTING: isize = 50;
        let mut i = a + 1;
        for _ in 0..MAX_STEPS {
            while i < b && !self.less(i, i - 1) {
                i += 1;
            }
            if i == b {
                return true;
            }
            if b - a < SHORTEST_SHIFTING {
                return false;
            }
            self.swap(i, i - 1);
            if i - a >= 2 {
                let mut j = i - 1;
                while j >= 1 {
                    if !self.less(j, j - 1) {
                        break;
                    }
                    self.swap(j, j - 1);
                    j -= 1;
                }
            }
            if b - i >= 2 {
                let mut j = i + 1;
                while j < b {
                    if !self.less(j, j - 1) {
                        break;
                    }
                    self.swap(j, j - 1);
                    j += 1;
                }
            }
        }
        false
    }

    fn break_patterns(&mut self, a: isize, b: isize) {
        let length = b - a;
        if length >= 8 {
            let mut random = u64::try_from(length).unwrap_or_default();
            let modulus = 1_u64 << bits_len(index(length));
            let mut idx = a + (length / 4) * 2 - 1;
            while idx <= a + (length / 4) * 2 + 1 {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                let mut other = isize::try_from(random & (modulus - 1)).unwrap_or_default();
                if other >= length {
                    other -= length;
                }
                self.swap(idx, a + other);
                idx += 1;
            }
        }
    }

    fn choose_pivot(&mut self, a: isize, b: isize) -> (isize, Hint) {
        const SHORTEST_NINTHER: isize = 50;
        const MAX_SWAPS: u32 = 4 * 3;
        let l = b - a;
        let mut swaps = 0;
        let mut i = a + l / 4;
        let mut j = a + l / 4 * 2;
        let mut k = a + l / 4 * 3;
        if l >= 8 {
            if l >= SHORTEST_NINTHER {
                i = self.median_adjacent(i, &mut swaps);
                j = self.median_adjacent(j, &mut swaps);
                k = self.median_adjacent(k, &mut swaps);
            }
            j = self.median(i, j, k, &mut swaps);
        }
        match swaps {
            0 => (j, Hint::Increasing),
            MAX_SWAPS => (j, Hint::Decreasing),
            _ => (j, Hint::Unknown),
        }
    }

    fn order2(&self, a: isize, b: isize, swaps: &mut u32) -> (isize, isize) {
        if self.less(b, a) {
            *swaps += 1;
            return (b, a);
        }
        (a, b)
    }

    fn median(&self, a: isize, b: isize, c: isize, swaps: &mut u32) -> isize {
        let (a, b) = self.order2(a, b, swaps);
        let (b, c) = self.order2(b, c, swaps);
        let _ = c;
        let (_, b) = self.order2(a, b, swaps);
        b
    }

    fn median_adjacent(&self, a: isize, swaps: &mut u32) -> isize {
        self.median(a - 1, a, a + 1, swaps)
    }

    fn reverse_range(&mut self, a: isize, b: isize) {
        let (mut i, mut j) = (a, b - 1);
        while i < j {
            self.swap(i, j);
            i += 1;
            j -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sort;

    #[test]
    fn sorts_like_a_total_order() {
        let mut values = (0..200).map(|i| (i * 7919) % 211).collect::<Vec<_>>();
        sort(&mut values, |a, b| a < b);
        assert!(values.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn short_slices_keep_equal_elements_in_order() {
        let mut values = (0..12).map(|i| (i % 3, i)).collect::<Vec<_>>();
        sort(&mut values, |a, b| a.0 < b.0);
        assert_eq!(
            values.iter().map(|value| value.1).collect::<Vec<_>>(),
            [0, 3, 6, 9, 1, 4, 7, 10, 2, 5, 8, 11]
        );
    }
}
