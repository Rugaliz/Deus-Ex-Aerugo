//! The engine's in-place sort (the templated quicksort the DLLs inline, e.g. Render.dll 10b0b830 for the actor's
//! lights). Its order for equal keys is part of the behaviour, so it is reproduced step by step: ranges of more than 8
//! items are split around their middle item, smaller ranges are finished by repeatedly moving the selected item to the
//! end; the larger part of a split is kept on a stack and the smaller one is sorted first.

/// Sorts `v` with `cmp(a, b)` (> 0 when `a` belongs after `b` among the selected-to-the-end items, as the engine's
/// `Compare`). The engine's comparisons never return 0 for the uses ported so far.
pub fn engine_sort<T>(v: &mut [T], cmp: impl Fn(&T, &T) -> i32) {
    if v.len() < 2 {
        return;
    }
    let mut stack: Vec<(usize, usize)> = vec![(0, v.len() - 1)];
    while let Some((mut lo, mut hi)) = stack.pop() {
        loop {
            let count = hi - lo + 1;
            if count <= 8 {
                // move the selected item of lo..=hi to hi, shrink, repeat
                while hi > lo {
                    let mut sel = lo;
                    for i in lo + 1..=hi {
                        if cmp(&v[i], &v[sel]) > 0 {
                            sel = i;
                        }
                    }
                    v.swap(sel, hi);
                    hi -= 1;
                }
                break;
            }
            // the middle item becomes the pivot at lo
            v.swap(lo + count / 2, lo);
            let (mut a, mut b) = (lo, hi + 1);
            loop {
                a += 1;
                while a <= hi && cmp(&v[a], &v[lo]) <= 0 {
                    a += 1;
                }
                b -= 1;
                while b > lo && cmp(&v[b], &v[lo]) >= 0 {
                    b -= 1;
                }
                if a > b {
                    break;
                }
                v.swap(a, b);
            }
            v.swap(lo, b);
            // keep the larger part for later, go on with the smaller one
            if (b as isize - 1 - lo as isize) < (hi as isize - a as isize) {
                if a < hi {
                    stack.push((a, hi));
                }
                if b > lo + 1 {
                    hi = b - 1;
                    continue;
                }
            } else {
                if lo + 1 < b {
                    stack.push((lo, b - 1));
                }
                if hi > a {
                    lo = a;
                    continue;
                }
            }
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The light sort's comparison (Render.dll 10b0b830): brightest `LightingTag` first, never equal.
    fn by_tag(a: &(i32, char), b: &(i32, char)) -> i32 {
        if b.0 < a.0 {
            -1
        } else {
            1
        }
    }

    #[test]
    fn small_ranges_keep_the_engines_order_for_equal_keys() {
        let mut v = vec![(5, 'a'), (5, 'b'), (3, 'c')];
        engine_sort(&mut v, by_tag);
        assert_eq!(v, vec![(5, 'a'), (5, 'b'), (3, 'c')]);
        let mut v = vec![(3, 'c'), (5, 'a'), (5, 'b')];
        engine_sort(&mut v, by_tag);
        assert_eq!(v, vec![(5, 'b'), (5, 'a'), (3, 'c')]);
    }

    #[test]
    fn large_ranges_come_out_sorted() {
        let mut seed = 12345u32;
        for n in [9usize, 10, 17, 40, 120] {
            let mut v: Vec<(i32, char)> = (0..n)
                .map(|_| {
                    seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                    (((seed >> 16) % 50) as i32, 'x')
                })
                .collect();
            engine_sort(&mut v, by_tag);
            assert!(v.windows(2).all(|w| w[0].0 >= w[1].0), "{v:?}");
        }
    }
}
