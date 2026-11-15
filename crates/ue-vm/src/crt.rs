//! C runtime routines whose exact behaviour shows in the game.
//!
//! `appQsort` (Core.dll 10123640) only forwards to `qsort` from the `MSVCRT.dll` the game ships in `System/`
//! (version 5.00.7303, `qsort` at 7802543d). That sort is not stable: the order it leaves equal items in is part of
//! the engine's behaviour (hits at the same time in `MultiLineCheck`, nodes at the same distance in the pathnode
//! list, senders with the same score in the event manager).

use std::cmp::Ordering;

/// Ranges of at most this many items are finished by `short_sort`.
const CUTOFF: usize = 8;

/// `qsort` as the shipped MSVCRT does it. `cmp(v, a, b)` compares the items at positions `a` and `b` and answers like
/// a C comparator (`< 0`, `0`, `> 0`); it gets the positions because one engine comparator breaks ties by them.
///
/// The algorithm, from the disassembly: an explicit stack of ranges; a range of `CUTOFF` items or fewer goes to
/// `short_sort`; otherwise the middle item (`lo + size / 2`) is swapped to the front and used as the pivot, `left`
/// moves up past items `<= pivot` and `right` down past items `>= pivot`, out-of-place pairs are swapped, and the pivot
/// is swapped into the gap. The smaller side is sorted next, the larger one pushed.
pub fn qsort<T>(v: &mut [T], mut cmp: impl FnMut(&[T], usize, usize) -> i32) {
    if v.len() < 2 {
        return;
    }
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let (mut lo, mut hi) = (0usize, v.len() - 1);
    loop {
        let size = hi - lo + 1;
        if size <= CUTOFF {
            short_sort(v, lo, hi, &mut cmp);
        } else {
            v.swap(lo + size / 2, lo);
            let mut left = lo;
            // one past the end; the scan below steps down before it compares
            let mut right = hi + 1;
            loop {
                loop {
                    left += 1;
                    if left > hi || cmp(v, left, lo) > 0 {
                        break;
                    }
                }
                loop {
                    right -= 1;
                    if right <= lo || cmp(v, right, lo) < 0 {
                        break;
                    }
                }
                if right < left {
                    break;
                }
                v.swap(left, right);
            }
            v.swap(lo, right);
            // `right` now holds the pivot: [lo, right - 1] and [left, hi] are left to sort. The sizes are compared
            // signed (`left` can be `hi + 1`).
            if right as isize - lo as isize > hi as isize - left as isize {
                if lo + 1 < right {
                    stack.push((lo, right - 1));
                }
                if left < hi {
                    lo = left;
                    continue;
                }
            } else {
                if left < hi {
                    stack.push((left, hi));
                }
                if lo + 1 < right {
                    hi = right - 1;
                    continue;
                }
            }
        }
        match stack.pop() {
            Some((l, h)) => (lo, hi) = (l, h),
            None => return,
        }
    }
}

/// MSVCRT's `shortsort` (780255e2): selection sort that finds the first largest item of `[lo, hi]` (a later item only
/// replaces it when strictly greater) and swaps it to `hi`, then shrinks the range.
fn short_sort<T>(v: &mut [T], lo: usize, mut hi: usize, cmp: &mut impl FnMut(&[T], usize, usize) -> i32) {
    while hi > lo {
        let mut max = lo;
        for p in lo + 1..=hi {
            if cmp(v, p, max) > 0 {
                max = p;
            }
        }
        v.swap(max, hi);
        hi -= 1;
    }
}

/// The engine's float comparators (`fcomp` then `C0`, then `C0|C3`): `-1` when `a < b` or either is NaN, `1` when
/// `a > b`, else `0`.
pub fn x87_cmp(a: f32, b: f32) -> i32 {
    match a.partial_cmp(&b) {
        Some(Ordering::Less) | None => -1,
        Some(Ordering::Greater) => 1,
        Some(Ordering::Equal) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sort_keys(keys: &[i32]) -> Vec<(i32, usize)> {
        let mut v: Vec<(i32, usize)> = keys.iter().copied().zip(0..).collect();
        qsort(&mut v, |v, a, b| v[a].0.cmp(&v[b].0) as i32);
        v
    }

    #[test]
    fn sorts_like_a_sort() {
        // a cheap LCG for varied inputs, all sizes around the cutoff and the stack logic
        let mut s = 12345u32;
        for n in 0..200 {
            let keys: Vec<i32> = (0..n)
                .map(|_| {
                    s = s.wrapping_mul(214013).wrapping_add(2531011);
                    ((s >> 16) % 13) as i32
                })
                .collect();
            let got: Vec<i32> = sort_keys(&keys).into_iter().map(|k| k.0).collect();
            let mut want = keys.clone();
            want.sort();
            assert_eq!(got, want, "n = {n}");
        }
    }

    #[test]
    fn short_sort_moves_the_first_maximum_last() {
        // worked by hand from the disassembly: [a0 a1 a2] all equal -> first pass swaps a0 with a2 ([a2 a1 a0]), the
        // second pass swaps a2 with a1 ([a1 a2 a0])
        let got: Vec<usize> = sort_keys(&[5, 5, 5]).into_iter().map(|k| k.1).collect();
        assert_eq!(got, vec![1, 2, 0]);
        let got: Vec<usize> = sort_keys(&[5, 5]).into_iter().map(|k| k.1).collect();
        assert_eq!(got, vec![1, 0]);
        // [1a 0 1b]: max = 1a (1b is not greater), swapped to the end -> [1b 0 1a]; then [1b 0] -> [0 1b]
        let got: Vec<usize> = sort_keys(&[1, 0, 1]).into_iter().map(|k| k.1).collect();
        assert_eq!(got, vec![1, 2, 0]);
    }

    #[test]
    fn partition_step_by_hand() {
        // nine equal keys (one more than the cutoff), traced by hand through the disassembly: the middle item 4 is
        // swapped to the front ([4 1 2 3 0 5 6 7 8]); `left` passes every item (all <= pivot) to 9, `right` every item
        // down to `lo` (0); the pivot swap is a no-op; the sides are [0, -1] and [9, 8], both empty: done
        let got: Vec<usize> = sort_keys(&[7; 9]).into_iter().map(|k| k.1).collect();
        assert_eq!(got, vec![4, 1, 2, 3, 0, 5, 6, 7, 8]);
        // ten items, pivot 2 (middle 5 holds key 2): [5 1 9 0 3 2 8 4 7 6] keys [2 1 9 0 3 5 8 4 7 6] after the swap.
        // left stops at 2 (9 > 2), right stops at 3 (0 < 2): swap -> keys [2 1 0 9 3 5 8 4 7 6]; left stops at 3,
        // right at 2: cross. Pivot swap(0, 2) -> [0 1 2 9 3 5 8 4 7 6]. Sides [0, 1] (size 2) and [3, 9] (size 7):
        // 1 > 6 is false, so [3, 9] is pushed and [0, 1] sorted first; both short-sorted.
        let got: Vec<i32> = sort_keys(&[5, 1, 9, 0, 3, 2, 8, 4, 7, 6]).into_iter().map(|k| k.0).collect();
        assert_eq!(got, (0..10).collect::<Vec<_>>());
    }

    /// Orders produced by the shipped `MSVCRT.dll`'s `qsort` itself, run in the private x86 emulator on `{key, id}`
    /// items with a `key a - key b` comparator (`keys | ids in sorted order` per line).
    #[test]
    fn matches_the_dll() {
        for line in include_str!("../tests/data/msvcrt_qsort.txt").lines() {
            let (k, ids) = line.split_once(" | ").unwrap();
            let keys: Vec<i32> = k.split(' ').map(|x| x.parse().unwrap()).collect();
            let want: Vec<usize> = ids.split(' ').map(|x| x.parse().unwrap()).collect();
            let got: Vec<usize> = sort_keys(&keys).into_iter().map(|k| k.1).collect();
            assert_eq!(got, want, "keys {k}");
        }
    }

    #[test]
    fn x87_compare() {
        assert_eq!(x87_cmp(1.0, 2.0), -1);
        assert_eq!(x87_cmp(2.0, 1.0), 1);
        assert_eq!(x87_cmp(0.0, -0.0), 0);
        assert_eq!(x87_cmp(f32::NAN, 1.0), -1);
        assert_eq!(x87_cmp(1.0, f32::NAN), -1);
    }
}

/// msvcrt `%.<prec>f` of a double: 17 significant digits (correctly rounded), that digit string rounded half up at
/// the precision, zeros beyond; infinities and NaNs as `1.#INF00`-style text (`1.#QNAN0`, `-1.#IND00`).
pub fn fixed(d: f64, prec: usize) -> String {
    let sign = if d.is_sign_negative() { "-" } else { "" };
    if d.is_nan() {
        return if d.is_sign_negative() { "-1.#IND00".into() } else { "1.#QNAN0".into() };
    }
    if d.is_infinite() {
        return format!("{sign}1.#INF00");
    }
    let frac_zeros = |n: usize| "0".repeat(n);
    if d == 0.0 {
        return if prec == 0 { format!("{sign}0") } else { format!("{sign}0.{}", frac_zeros(prec)) };
    }
    let e = format!("{:.16e}", d.abs());
    let (mant, exp) = e.split_once('e').expect("exponent");
    let mut digits: Vec<u8> = mant.bytes().filter(u8::is_ascii_digit).map(|b| b - b'0').collect();
    let mut point = exp.parse::<i32>().expect("exponent") + 1;
    let keep = point + prec as i32;
    if keep < 0 {
        digits.clear();
    } else if (keep as usize) < digits.len() {
        let up = digits[keep as usize] >= 5;
        digits.truncate(keep as usize);
        if up {
            let mut i = digits.len();
            loop {
                if i == 0 {
                    digits.insert(0, 1);
                    point += 1;
                    break;
                }
                i -= 1;
                if digits[i] == 9 {
                    digits[i] = 0;
                } else {
                    digits[i] += 1;
                    break;
                }
            }
        }
    }
    let at = |k: i32| if k >= 0 && (k as usize) < digits.len() { (b'0' + digits[k as usize]) as char } else { '0' };
    let int: String = if point <= 0 { "0".into() } else { (0..point).map(at).collect() };
    if prec == 0 {
        return format!("{sign}{int}");
    }
    let frac: String = (point..point + prec as i32).map(at).collect();
    format!("{sign}{int}.{frac}")
}

/// msvcrt `sprintf(fmt, d)` with one double argument, as the window system formats a value (`ScaleWindow`'s value
/// format). The arguments are the double's 8 bytes then zeros: `%d`/`%i`/`%u`/`%x`/`%X`/`%o`/`%c` take the next 4
/// bytes (the first `%d` reads the double's low word), `%f`/`%e`/`%g` the next 8. Flags `-` `+` space `0`, width,
/// precision.
pub fn sprintf_double(fmt: &str, d: f64) -> String {
    let bits = d.to_bits();
    let words = [bits as u32, (bits >> 32) as u32];
    let mut next = 0usize;
    let take4 = |n: &mut usize| {
        let w = words.get(*n).copied().unwrap_or(0);
        *n += 1;
        w
    };
    let mut out = String::new();
    let mut it = fmt.chars().peekable();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let (mut left, mut plus, mut space, mut zero) = (false, false, false, false);
        while let Some(&f) = it.peek() {
            match f {
                '-' => left = true,
                '+' => plus = true,
                ' ' => space = true,
                '0' => zero = true,
                '#' => {}
                _ => break,
            }
            it.next();
        }
        let mut width = 0usize;
        while let Some(&n) = it.peek().filter(|c| c.is_ascii_digit()) {
            width = width * 10 + n.to_digit(10).unwrap_or(0) as usize;
            it.next();
        }
        let mut prec: Option<usize> = None;
        if it.peek() == Some(&'.') {
            it.next();
            let mut p = 0usize;
            while let Some(&n) = it.peek().filter(|c| c.is_ascii_digit()) {
                p = p * 10 + n.to_digit(10).unwrap_or(0) as usize;
                it.next();
            }
            prec = Some(p);
        }
        while matches!(it.peek(), Some('l') | Some('h') | Some('L')) {
            it.next();
        }
        let sign_of = |neg: bool| if neg { "-" } else if plus { "+" } else if space { " " } else { "" };
        let (body, signed) = match it.next() {
            Some('%') => {
                out.push('%');
                continue;
            }
            Some('d') | Some('i') => {
                let v = take4(&mut next) as i32;
                let digits = (v as i64).unsigned_abs().to_string();
                (format!("{}{digits}", sign_of(v < 0)), true)
            }
            Some('u') => (take4(&mut next).to_string(), true),
            Some('x') => (format!("{:x}", take4(&mut next)), true),
            Some('X') => (format!("{:X}", take4(&mut next)), true),
            Some('o') => (format!("{:o}", take4(&mut next)), true),
            Some('c') => (char::from_u32(take4(&mut next) & 0xff).map(String::from).unwrap_or_default(), false),
            Some(conv @ ('f' | 'e' | 'E' | 'g' | 'G')) => {
                let lo = take4(&mut next) as u64;
                let hi = take4(&mut next) as u64;
                let v = f64::from_bits(lo | (hi << 32));
                let p = prec.unwrap_or(6);
                let s = match conv {
                    'f' => fixed(v, p),
                    'e' | 'E' => {
                        let s = format!("{:.*e}", p, v);
                        let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
                        let e: i32 = e.parse().unwrap_or(0);
                        let t = format!("{m}e{}{:03}", if e < 0 { '-' } else { '+' }, e.abs());
                        if conv == 'E' { t.to_uppercase() } else { t }
                    }
                    _ => {
                        let s = format!("{}", v);
                        if conv == 'G' { s.to_uppercase() } else { s }
                    }
                };
                let s = if !s.starts_with('-') && (plus || space) { format!("{}{s}", sign_of(false)) } else { s };
                (s, true)
            }
            Some(o) => {
                out.push('%');
                out.push(o);
                continue;
            }
            None => {
                out.push('%');
                continue;
            }
        };
        let len = body.chars().count();
        if len >= width {
            out.push_str(&body);
        } else if left {
            out.push_str(&body);
            out.push_str(&" ".repeat(width - len));
        } else if zero && signed {
            let (sgn, rest) = match body.chars().next() {
                Some(s @ ('-' | '+' | ' ')) => (s.to_string(), body[1..].to_string()),
                _ => (String::new(), body.clone()),
            };
            out.push_str(&sgn);
            out.push_str(&"0".repeat(width - len));
            out.push_str(&rest);
        } else {
            out.push_str(&" ".repeat(width - len));
            out.push_str(&body);
        }
    }
    out
}

#[cfg(test)]
mod format_tests {
    use super::*;

    #[test]
    fn printf_of_one_double() {
        // `%d` reads the double's low word (40.0 has none set)
        assert_eq!(sprintf_double("%d", 40.0), "0");
        assert_eq!(sprintf_double("%5.2f", 40.0), "40.00");
        assert_eq!(sprintf_double("%.1f%%", 2.25), "2.3%");
        assert_eq!(sprintf_double("%-6.1f|", 1.5), "1.5   |");
        assert_eq!(fixed(0.125, 2), "0.13");
        assert_eq!(fixed(-0.0, 1), "-0.0");
    }
}

/// A value held as `hi + lo` (double-double), for the few x87 computations whose extended precision shows.
#[derive(Debug, Clone, Copy)]
struct Dd(f64, f64);

/// `hi + lo` rounded to the x87's 64-bit mantissa (to nearest): `lo` becomes a multiple of the extended ulp of `hi`.
fn round_ext(v: Dd) -> Dd {
    if v.0 == 0.0 || !v.0.is_finite() {
        return v;
    }
    let ulp = 2f64.powi(v.0.abs().log2().floor() as i32 - 63);
    Dd(v.0, (v.1 / ulp).round_ties_even() * ulp)
}

/// `atan2(y, x)` as the x87's `fpatan` gives it (64-bit mantissa). Near an axis (|small/large| < 2^-20) the result
/// is a multiple of pi/2 plus a tiny term that a double cannot hold next to it; there it is worked out with
/// double-double constants. Elsewhere a double is exact enough for every caller (they truncate after a scaling).
fn fpatan(y: f64, x: f64) -> Dd {
    const PI_2: Dd = Dd(1.5707963267948966, 6.123233995736766e-17);
    const PI: Dd = Dd(3.141592653589793, 1.2246467991473532e-16);
    let (ay, ax) = (y.abs(), x.abs());
    if ay > 0.0 && ax < ay * 9.5367431640625e-7 {
        // +-pi/2 - atan(x/y), atan(t) = t - t^3/3 for tiny t
        let t = x / y;
        let a = t - t * t * t / 3.0;
        let s = if y > 0.0 { 1.0 } else { -1.0 };
        return round_ext(Dd(s * PI_2.0, s * PI_2.1 - a));
    }
    if ax > 0.0 && ay < ax * 9.5367431640625e-7 {
        let t = y / x;
        let a = t - t * t * t / 3.0;
        if x > 0.0 {
            return round_ext(Dd(a, 0.0));
        }
        // +-pi + atan(y/x)
        let s = if y.is_sign_negative() { -1.0 } else { 1.0 };
        return round_ext(Dd(s * PI.0, s * PI.1 + a));
    }
    Dd(y.atan2(x), 0.0)
}

/// `_ftol(appAtan2(y, x) * scale)` with the product on the x87 stack (64-bit mantissa) and truncated: the rotator
/// units of an angle (`FCoords::OrthoRotation`'s scale 32768/pi).
pub fn x87_atan2_units(y: f64, x: f64, scale: f64) -> i32 {
    let a = fpatan(y, x);
    let p = a.0 * scale;
    let e = a.0.mul_add(scale, -p) + a.1 * scale;
    let v = round_ext(Dd(p, e));
    // truncate hi + lo
    let t = v.0.trunc();
    let frac = (v.0 - t) + v.1;
    let r = if v.0 >= 0.0 {
        if frac < 0.0 {
            t - 1.0
        } else if frac >= 1.0 {
            t + 1.0
        } else {
            t
        }
    } else if frac > 0.0 {
        t + 1.0
    } else if frac <= -1.0 {
        t - 1.0
    } else {
        t
    };
    crate::route::ftol(r)
}

#[cfg(test)]
mod x87_tests {
    use super::*;

    #[test]
    fn straight_up_truncates_below_a_quarter_turn() {
        // the sine table's cos(16384) is sin(pi) as a float: 1.2246469e-16
        let x = 1.2246469e-16f32 as f64;
        assert_eq!(x87_atan2_units(1.0, x, 10430.378350470453), 16383);
        assert_eq!(x87_atan2_units(1.0, 0.0, 10430.378350470453), 16384);
        assert_eq!(x87_atan2_units(0.5, 0.5, 10430.378350470453), 8192);
        assert_eq!(x87_atan2_units(-1.0, x, 10430.378350470453), -16383);
    }
}
