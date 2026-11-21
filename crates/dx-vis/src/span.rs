//! `FSpanBuffer`: per screen row, the sorted list of open half-open x ranges `[start, end)`. The walk keeps one per
//! zone (what is still uncovered in that zone); solid surfaces close what they cover, sprites only test against it.
//! Only the covered set matters to the engine's results (and whether any span is left), so the rows are kept as
//! normalised interval lists rather than the engine's linked lists.

/// One open range of a row.
pub type Span = (i32, i32);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpanBuffer {
    /// First row and one past the last row (`StartY`, `EndY`).
    pub start_y: i32,
    pub end_y: i32,
    rows: Vec<Vec<Span>>,
    /// Bit `i` set when row `start_y + i` has an open span: tests skip closed rows a word at a time.
    open: Vec<u64>,
}

/// Replaces the run `row[first..last]` of spans that `[a, b)` overlaps by their parts outside it.
#[inline]
fn close_run(row: &mut Vec<Span>, first: usize, last: usize, a: i32, b: i32) {
    let (s0, e1) = (row[first].0, row[last - 1].1);
    let left = (s0 < a).then_some((s0, a));
    let right = (e1 > b).then_some((b, e1));
    let keep = left.is_some() as usize + right.is_some() as usize;
    let mut k = first;
    for p in left.into_iter().chain(right) {
        if k < last {
            row[k] = p;
        } else {
            row.insert(k, p);
        }
        k += 1;
    }
    if first + keep < last {
        row.drain(first + keep..last);
    }
}

impl SpanBuffer {
    /// `AllocIndex(Y1, Y2)`: rows `y1..y2`, all closed.
    pub fn alloc(y1: i32, y2: i32) -> SpanBuffer {
        let n = (y2 - y1).max(0) as usize;
        SpanBuffer { start_y: y1, end_y: y1.max(y2), rows: vec![Vec::new(); n], open: vec![0; n.div_ceil(64)] }
    }

    /// `alloc(y1, y2)` in place: the same buffer, keeping the rows' allocations (the walk reuses one per surface).
    pub fn reset(&mut self, y1: i32, y2: i32) {
        let n = (y2 - y1).max(0) as usize;
        self.start_y = y1;
        self.end_y = y1.max(y2);
        self.rows.truncate(n);
        for r in self.rows.iter_mut() {
            r.clear();
        }
        self.rows.resize_with(n, Vec::new);
        self.open.clear();
        self.open.resize(n.div_ceil(64), 0);
    }

    /// `AllocIndexForScreen(X, Y)`: rows `0..height`, each open over `0..width`.
    pub fn for_screen(width: i32, height: i32) -> SpanBuffer {
        let h = height.max(0);
        let mut b = SpanBuffer { start_y: 0, end_y: h, rows: vec![if width > 0 { vec![(0, width)] } else { Vec::new() }; h as usize], open: vec![0; (h as usize).div_ceil(64)] };
        if width > 0 {
            for i in 0..h as usize {
                b.open[i / 64] |= 1 << (i % 64);
            }
        }
        b
    }

    /// `ValidLines`: the number of open spans (the engine tests it against 0).
    pub fn valid(&self) -> usize {
        self.rows.iter().map(|r| r.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.open.iter().all(|&w| w == 0)
    }

    /// Brings row index `i`'s bit in `open` in line with the row.
    #[inline]
    fn sync(&mut self, i: usize) {
        let bit = 1u64 << (i % 64);
        if self.rows[i].is_empty() {
            self.open[i / 64] &= !bit;
        } else {
            self.open[i / 64] |= bit;
        }
    }

    fn clear_rows(&mut self) {
        for r in self.rows.iter_mut() {
            r.clear();
        }
        self.open.iter_mut().for_each(|w| *w = 0);
    }

    /// The open spans of row `y` (empty outside the buffer).
    pub fn row(&self, y: i32) -> &[Span] {
        if y < self.start_y || y >= self.end_y {
            return &[];
        }
        &self.rows[(y - self.start_y) as usize]
    }

    /// `CopyFromRaster(Source, RY1, RY2, Raster)`: this buffer becomes the part of `raster` (one span per row, rows
    /// `ry1..ry2`) that is open in `source`, over this buffer's rows. True when anything is open.
    pub fn copy_from_raster(&mut self, source: &SpanBuffer, ry1: i32, ry2: i32, raster: &[Span]) -> bool {
        self.copy_raster(source, ry1, ry2, raster)
    }

    /// `CopyFromRasterUpdate`: like `copy_from_raster`, and what was open is closed in `source` (a solid surface hides
    /// what lies behind it).
    pub fn copy_from_raster_update(&mut self, source: &mut SpanBuffer, ry1: i32, ry2: i32, raster: &[Span]) -> bool {
        self.clear_rows();
        let y1 = source.start_y.max(ry1).max(self.start_y);
        let y2 = source.end_y.min(ry2).min(self.end_y);
        let mut any = false;
        for y in y1..y2 {
            let Some(&(rs, re)) = raster.get((y - ry1) as usize) else { continue };
            if rs >= re {
                continue;
            }
            // the source's spans are sorted and apart: the ones `[rs, re)` overlaps are a run, copied clipped to it and
            // then replaced by their parts outside it (`close`)
            let si = (y - source.start_y) as usize;
            let row = &mut source.rows[si];
            let first = row.iter().position(|&(_, e)| e > rs).unwrap_or(row.len());
            let last = row[first..].iter().position(|&(s, _)| s >= re).map(|k| first + k).unwrap_or(row.len());
            if first >= last {
                continue;
            }
            let i = (y - self.start_y) as usize;
            let out = &mut self.rows[i];
            out.extend(row[first..last].iter().map(|&(s, e)| (s.max(rs), e.min(re))));
            any = true;
            self.open[i / 64] |= 1u64 << (i % 64);
            close_run(row, first, last, rs, re);
            source.sync(si);
        }
        any
    }

    fn copy_raster(&mut self, source: &SpanBuffer, ry1: i32, ry2: i32, raster: &[Span]) -> bool {
        self.clear_rows();
        let y1 = source.start_y.max(ry1).max(self.start_y);
        let y2 = source.end_y.min(ry2).min(self.end_y);
        let mut any = false;
        for y in y1..y2 {
            let Some(&(rs, re)) = raster.get((y - ry1) as usize) else { continue };
            if rs >= re {
                continue;
            }
            let i = (y - self.start_y) as usize;
            let out = &mut self.rows[i];
            for &(s, e) in source.row(y) {
                let (a, b) = (s.max(rs), e.min(re));
                if a < b {
                    out.push((a, b));
                    any = true;
                }
            }
            self.sync(i);
        }
        any
    }

    /// Closes `[a, b)` in row `y`.
    pub fn close(&mut self, y: i32, a: i32, b: i32) {
        if y < self.start_y || y >= self.end_y || a >= b {
            return;
        }
        let i = (y - self.start_y) as usize;
        let row = &mut self.rows[i];
        // the spans are sorted and apart: the ones `[a, b)` touches are a run, replaced by their parts outside it
        let first = row.iter().position(|&(_, e)| e > a).unwrap_or(row.len());
        let last = row[first..].iter().position(|&(s, _)| s >= b).map(|k| first + k).unwrap_or(row.len());
        if first >= last {
            return;
        }
        close_run(row, first, last, a, b);
        self.sync(i);
    }

    /// Closes the rectangle `x1..x2` x `y1..y2` (tests).
    pub fn close_rect(&mut self, x1: i32, y1: i32, x2: i32, y2: i32) {
        for y in y1..y2 {
            self.close(y, x1, x2);
        }
    }

    /// `MergeWith(Other)`: the union; the row range grows to cover both.
    pub fn merge_with(&mut self, other: &SpanBuffer) {
        if other.start_y >= other.end_y {
            return;
        }
        if self.start_y >= self.end_y {
            *self = other.clone();
            return;
        }
        let (y1, y2) = (self.start_y.min(other.start_y), self.end_y.max(other.end_y));
        if y1 != self.start_y || y2 != self.end_y {
            let mut rows = vec![Vec::new(); (y2 - y1) as usize];
            for (i, r) in self.rows.drain(..).enumerate() {
                rows[(self.start_y - y1) as usize + i] = r;
            }
            self.rows = rows;
            self.start_y = y1;
            self.end_y = y2;
            self.open = vec![0; self.rows.len().div_ceil(64)];
            for i in 0..self.rows.len() {
                self.sync(i);
            }
        }
        for y in other.start_y..other.end_y {
            let add = other.row(y);
            if add.is_empty() {
                continue;
            }
            let row = &mut self.rows[(y - self.start_y) as usize];
            row.extend_from_slice(add);
            row.sort_unstable();
            let mut merged: Vec<Span> = Vec::with_capacity(row.len());
            for &(s, e) in row.iter() {
                match merged.last_mut() {
                    Some(last) if s <= last.1 => last.1 = last.1.max(e),
                    _ => merged.push((s, e)),
                }
            }
            *row = merged;
            self.sync((y - self.start_y) as usize);
        }
    }

    /// `BoxIsVisible(X1, Y1, X2, Y2)`: any open span in rows `y1..y2` overlapping `x1..x2`.
    pub fn box_is_visible(&self, x1: i32, y1: i32, x2: i32, y2: i32) -> bool {
        let (a, b) = (y1.max(self.start_y), y2.min(self.end_y));
        if a >= b {
            return false;
        }
        // only the rows with something open
        let (lo, hi) = ((a - self.start_y) as usize, (b - self.start_y) as usize);
        let mut w = lo / 64;
        while w * 64 < hi {
            let mut bits = self.open[w];
            if w == lo / 64 {
                bits &= !0u64 << (lo % 64);
            }
            if (w + 1) * 64 > hi {
                bits &= !0u64 >> (64 - (hi - w * 64));
            }
            while bits != 0 {
                let i = w * 64 + bits.trailing_zeros() as usize;
                if self.rows[i].iter().any(|&(s, e)| s < x2 && e > x1) {
                    return true;
                }
                bits &= bits - 1;
            }
            w += 1;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Coverage as a set of pixels, the independent reference.
    fn pixels(b: &SpanBuffer) -> std::collections::BTreeSet<(i32, i32)> {
        let mut s = std::collections::BTreeSet::new();
        for y in b.start_y..b.end_y {
            for &(a, e) in b.row(y) {
                for x in a..e {
                    s.insert((x, y));
                }
            }
        }
        s
    }

    #[test]
    fn a_solid_surface_hides_what_is_behind_it() {
        let mut zone = SpanBuffer::for_screen(10, 4);
        // a "wall" covering x 2..6 on rows 1..3
        let mut wall = SpanBuffer::alloc(1, 3);
        assert!(wall.copy_from_raster_update(&mut zone, 1, 3, &[(2, 6), (2, 6)]));
        assert_eq!(pixels(&wall).len(), 8);
        assert!(!zone.box_is_visible(3, 1, 5, 3), "behind the wall");
        assert!(zone.box_is_visible(5, 1, 7, 3), "beside the wall");
        // a sprite fully behind it: nothing open
        let mut sprite = SpanBuffer::alloc(1, 3);
        assert!(!sprite.copy_from_raster(&zone, 1, 3, &[(3, 5), (3, 5)]));
        // testing does not close anything
        assert_eq!(zone.valid(), 2 + 2 * 2);
    }

    #[test]
    fn rows_outside_the_source_are_closed() {
        let zone = SpanBuffer::for_screen(8, 2);
        let mut s = SpanBuffer::alloc(0, 4);
        assert!(s.copy_from_raster(&zone, 1, 4, &[(0, 3), (1, 2), (0, 8)]));
        assert_eq!(s.row(1), &[(0, 3)]);
        assert!(s.row(2).is_empty() && s.row(3).is_empty() && s.row(0).is_empty());
    }

    #[test]
    fn merge_is_the_union() {
        let mut a = SpanBuffer::alloc(0, 1);
        a.copy_from_raster(&SpanBuffer::for_screen(20, 3), 0, 1, &[(0, 4)]);
        let mut b = SpanBuffer::alloc(1, 3);
        b.copy_from_raster(&SpanBuffer::for_screen(20, 3), 1, 3, &[(2, 6), (5, 9)]);
        let mut c = SpanBuffer::alloc(0, 1);
        c.copy_from_raster(&SpanBuffer::for_screen(20, 3), 0, 1, &[(3, 7)]);
        let want: std::collections::BTreeSet<_> = pixels(&a).union(&pixels(&b)).chain(pixels(&c).iter()).copied().collect();
        a.merge_with(&b);
        a.merge_with(&c);
        assert_eq!((a.start_y, a.end_y), (0, 3));
        assert_eq!(pixels(&a), want);
        assert_eq!(a.row(0), &[(0, 7)]);
    }
}
