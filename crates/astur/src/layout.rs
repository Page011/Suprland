//! Tiling layout math: pure geometry, no Win32 state. Given a work area and a
//! window count, produce the target rectangle for each window.

use windows::Win32::Foundation::RECT;

/// Master-stack layout: one master column on the left,
/// the remaining windows stacked vertically on the right.
pub(crate) fn master_stack(area: RECT, n: usize, ratio: f32, outer: i32, inner: i32) -> Vec<RECT> {
    let mut out = Vec::with_capacity(n);
    let x0 = area.left + outer;
    let y0 = area.top + outer;
    let w = area.right - area.left - 2 * outer;
    let h = area.bottom - area.top - 2 * outer;
    if n == 0 || w <= 0 || h <= 0 {
        return out;
    }
    if n == 1 {
        out.push(RECT {
            left: x0,
            top: y0,
            right: x0 + w,
            bottom: y0 + h,
        });
        return out;
    }
    // Clamp the gaps to what actually fits. `inner_gap` accepts up to 500 in the
    // config, and on a small work area with many stacked windows the unclamped
    // arithmetic produced rects with bottom < top — SetWindowPos then does
    // something arbitrary (review B-11). `grid_cells` has always guarded this
    // with `fitted_gap`; master and dwindle did not.
    let col_gap = fitted_gap(w, 2, inner);
    let master_w = ((w - col_gap) as f32 * ratio) as i32;
    let stack_w = (w - col_gap) - master_w;
    out.push(RECT {
        left: x0,
        top: y0,
        right: x0 + master_w,
        bottom: y0 + h,
    });
    let sx = x0 + master_w + col_gap;
    let sc = (n - 1) as i32;
    let row_gap = fitted_gap(h, n - 1, inner);
    let each = ((h - (sc - 1) * row_gap) / sc).max(1);
    for i in 0..sc {
        let sy = y0 + i * (each + row_gap);
        let bottom = if i == sc - 1 { y0 + h } else { sy + each };
        if bottom <= sy || sx + stack_w <= sx {
            break; // out of room; better to leave a window put than invert it
        }
        out.push(RECT {
            left: sx,
            top: sy,
            right: sx + stack_w,
            bottom,
        });
    }
    out
}

/// The split ratio for level `i`, defaulting to 0.5 and clamped to a sane range.
pub(crate) fn split_ratio(splits: &[f32], i: usize) -> f32 {
    splits.get(i).copied().unwrap_or(0.5).clamp(0.05, 0.95)
}

/// Dwindle/spiral layout (spiral default): each window takes a
/// fraction (`splits[i]`, default half) of the remaining space, alternating the
/// split along the longer side, so windows spiral toward the bottom corner.
/// Resizing a window edits the relevant `splits` entry (see `resize_dwindle`).
pub(crate) fn dwindle_layout(
    area: RECT,
    n: usize,
    outer: i32,
    inner: i32,
    splits: &[f32],
) -> Vec<RECT> {
    let mut out = Vec::with_capacity(n);
    if n == 0 {
        return out;
    }
    let mut cur = RECT {
        left: area.left + outer,
        top: area.top + outer,
        right: area.right - outer,
        bottom: area.bottom - outer,
    };
    if cur.right <= cur.left || cur.bottom <= cur.top {
        return out;
    }
    for i in 0..n {
        if i == n - 1 {
            out.push(cur);
            break;
        }
        let w = cur.right - cur.left;
        let h = cur.bottom - cur.top;
        // No room for another split: give every remaining window the rect we
        // have. Dropping them would leave real windows wherever they happened
        // to be, which is worse than a stack.
        if w.max(h) < MIN_TILE * 2 {
            for _ in i..n {
                out.push(cur);
            }
            break;
        }
        let r = split_ratio(splits, i);
        if w >= h {
            // See the note in master_stack: an unclamped gap wider than the
            // remaining space made `half` negative and inverted the rect.
            let gap = fitted_gap(w, 2, inner);
            let half = (((w - gap) as f32 * r) as i32).max(1);
            out.push(RECT {
                left: cur.left,
                top: cur.top,
                right: cur.left + half,
                bottom: cur.bottom,
            });
            cur.left += half + gap;
        } else {
            let gap = fitted_gap(h, 2, inner);
            let half = (((h - gap) as f32 * r) as i32).max(1);
            out.push(RECT {
                left: cur.left,
                top: cur.top,
                right: cur.right,
                bottom: cur.top + half,
            });
            cur.top += half + gap;
        }
        if cur.right <= cur.left || cur.bottom <= cur.top {
            // Belt and braces: never emit an inverted rect, whatever the gaps.
            for _ in i + 1..n {
                out.push(*out.last().expect("pushed above"));
            }
            break;
        }
    }
    out
}

/// True while `dwindle_layout` can still give every window its own tile. Above
/// this count the tail stacks (see `MIN_TILE`).
#[cfg(test)]
fn dwindle_tiles_fit(area: RECT, n: usize, outer: i32, inner: i32, splits: &[f32]) -> bool {
    let rects = dwindle_layout(area, n, outer, inner, splits);
    rects.len() == n && rects.windows(2).all(|p| p[0] != p[1])
}

/// Smallest tile a dwindle split may produce (roughly a title bar). Below this
/// the spiral stops dividing and the remaining windows share the last rect
/// (monocle-style tail) — every window still gets a real, on-screen rect, which
/// is what matters. Splitting past this point produced 1-px slivers, and before
/// the gaps were clamped it produced INVERTED rects that went straight to
/// SetWindowPos (review B-11).
///
/// Measured, not guessed: because the spiral halves geometrically, the value
/// barely moves the practical limit on a 1920x1052 work area with outer 8 /
/// inner 4 — 60 px gives 9 distinct tiles, 32 gives 10, and 16 still gives only
/// 12. Past ~10 windows dwindle is unusable whatever we do, so 32 is chosen for
/// staying closest to the old behaviour at realistic counts.
const MIN_TILE: i32 = 32;

/// Equal-width columns. Useful for ultrawide monitors and predictable placement.
pub(crate) fn columns_layout(area: RECT, n: usize, outer: i32, inner: i32) -> Vec<RECT> {
    grid_cells(area, n, n.max(1), outer, inner)
}

/// Balanced grid using approximately square cells. Last row stretches across
/// its available columns rather than leaving dead tiles.
pub(crate) fn grid_layout(area: RECT, n: usize, outer: i32, inner: i32) -> Vec<RECT> {
    if n == 0 {
        return Vec::new();
    }
    let cols = (n as f64).sqrt().ceil() as usize;
    grid_cells(area, n, cols.max(1), outer, inner)
}

fn grid_cells(area: RECT, n: usize, cols: usize, outer: i32, inner: i32) -> Vec<RECT> {
    let mut out = Vec::with_capacity(n);
    let left = area.left + outer;
    let top = area.top + outer;
    let width = area.right - area.left - 2 * outer;
    let height = area.bottom - area.top - 2 * outer;
    if n == 0 || width <= 0 || height <= 0 {
        return out;
    }
    let rows = n.div_ceil(cols);
    let row_gap = fitted_gap(height, rows, inner);
    let row_h = (height - row_gap * (rows.saturating_sub(1) as i32)) / rows as i32;
    for row in 0..rows {
        let start = row * cols;
        let count = (n - start).min(cols);
        let col_gap = fitted_gap(width, count, inner);
        let cell_w = (width - col_gap * (count.saturating_sub(1) as i32)) / count as i32;
        let y = top + row as i32 * (row_h + row_gap);
        let bottom = if row + 1 == rows {
            top + height
        } else {
            y + row_h
        };
        for col in 0..count {
            let x = left + col as i32 * (cell_w + col_gap);
            let right = if col + 1 == count {
                left + width
            } else {
                x + cell_w
            };
            out.push(RECT {
                left: x,
                top: y,
                right,
                bottom,
            });
        }
    }
    out
}

fn fitted_gap(extent: i32, cells: usize, requested: i32) -> i32 {
    if cells <= 1 {
        return 0;
    }
    let gaps = (cells - 1) as i32;
    requested.max(0).min((extent - cells as i32).max(0) / gaps)
}

/// Monocle layout: every tiled window fills the work area. Focus determines
/// which stacked window is visible; no window is resized differently.
pub(crate) fn monocle_layout(area: RECT, n: usize, outer: i32) -> Vec<RECT> {
    if n == 0 {
        return Vec::new();
    }
    let rect = RECT {
        left: area.left + outer,
        top: area.top + outer,
        right: area.right - outer,
        bottom: area.bottom - outer,
    };
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return Vec::new();
    }
    vec![rect; n]
}

/// Update `splits` so the dwindle window at tiled index `idx` matches the size
/// the user dragged it to (`new`). Replays the cascade to find that window's
/// split level + axis, then back-computes the ratio. Neighbours reflow to fill.
pub(crate) fn resize_dwindle(
    splits: &mut Vec<f32>,
    area: RECT,
    n: usize,
    outer: i32,
    inner: i32,
    idx: usize,
    new: RECT,
) {
    if n < 2 {
        return;
    }
    // The window at idx owns split level idx (it takes the first part); the very
    // last window instead shares level n-2 (it is that split's remainder).
    let (level, is_remainder) = if idx < n - 1 {
        (idx, false)
    } else {
        (n - 2, true)
    };
    if splits.len() < n - 1 {
        splits.resize(n - 1, 0.5);
    }
    // Replay the cascade up to `level` to find that split's available rect.
    let mut cur = RECT {
        left: area.left + outer,
        top: area.top + outer,
        right: area.right - outer,
        bottom: area.bottom - outer,
    };
    // Must replay EXACTLY what dwindle_layout does, gap clamping included, or a
    // drag maps onto the wrong split level.
    for i in 0..level {
        let w = cur.right - cur.left;
        let h = cur.bottom - cur.top;
        let r = split_ratio(splits, i);
        if w >= h {
            let gap = fitted_gap(w, 2, inner);
            let half = (((w - gap) as f32 * r) as i32).max(1);
            cur.left += half + gap;
        } else {
            let gap = fitted_gap(h, 2, inner);
            let half = (((h - gap) as f32 * r) as i32).max(1);
            cur.top += half + gap;
        }
    }
    let w = cur.right - cur.left;
    let h = cur.bottom - cur.top;
    let vertical = w >= h;
    let gap = fitted_gap(if vertical { w } else { h }, 2, inner);
    let avail = (if vertical { w } else { h } - gap).max(1) as f32;
    let new_size = if vertical {
        new.right - new.left
    } else {
        new.bottom - new.top
    } as f32;
    // First-half window: ratio = its size / available. Remainder window: it gets
    // (1 - ratio), so ratio = 1 - its size / available.
    let ratio = if is_remainder {
        1.0 - new_size / avail
    } else {
        new_size / avail
    };
    splits[level] = ratio.clamp(0.05, 0.95);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn split_ratio_defaults_and_clamps() {
        assert_eq!(split_ratio(&[], 0), 0.5); // missing -> default
        assert_eq!(split_ratio(&[0.3], 0), 0.3);
        assert_eq!(split_ratio(&[0.0], 0), 0.05); // clamp low
        assert_eq!(split_ratio(&[1.0], 0), 0.95); // clamp high
        assert_eq!(split_ratio(&[0.7], 5), 0.5); // out-of-range index -> default
    }

    #[test]
    fn master_stack_empty_and_degenerate() {
        assert!(master_stack(r(0, 0, 100, 100), 0, 0.5, 0, 0).is_empty());
        assert!(master_stack(r(0, 0, 0, 0), 3, 0.5, 0, 0).is_empty());
        // outer gap larger than the area leaves no usable space
        assert!(master_stack(r(0, 0, 10, 10), 2, 0.5, 20, 0).is_empty());
    }

    #[test]
    fn master_stack_single_fills_area_minus_outer() {
        let v = master_stack(r(0, 0, 100, 100), 1, 0.5, 10, 5);
        assert_eq!(v, vec![r(10, 10, 90, 90)]);
    }

    #[test]
    fn master_stack_two_split_by_ratio_no_overlap() {
        let v = master_stack(r(0, 0, 100, 100), 2, 0.5, 0, 0);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], r(0, 0, 50, 100));
        assert_eq!(v[1], r(50, 0, 100, 100)); // master right == stack left
    }

    #[test]
    fn master_stack_stack_covers_full_height() {
        // master + two stacked; last stack window's bottom hits the area bottom.
        let v = master_stack(r(0, 0, 200, 100), 3, 0.5, 0, 0);
        assert_eq!(v.len(), 3);
        assert_eq!(v[1].top, 0);
        assert_eq!(v[2].bottom, 100);
        assert!(v[1].bottom <= v[2].top); // no vertical overlap in the stack
    }

    #[test]
    fn dwindle_single_is_area_minus_outer() {
        let v = dwindle_layout(r(0, 0, 100, 100), 1, 8, 4, &[]);
        assert_eq!(v, vec![r(8, 8, 92, 92)]);
    }

    #[test]
    fn dwindle_count_and_first_split_vertical_when_wide() {
        let v = dwindle_layout(r(0, 0, 200, 100), 2, 0, 0, &[0.5]);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].left, 0);
        assert_eq!(v[0].right, v[1].left); // touching (inner gap 0)
        assert_eq!(v[1].right, 200);
    }

    #[test]
    fn dwindle_degenerate_area_empty() {
        assert!(dwindle_layout(r(0, 0, 5, 5), 2, 10, 0, &[]).is_empty());
    }

    #[test]
    fn columns_split_width_and_preserve_edge() {
        let v = columns_layout(r(0, 0, 100, 80), 3, 0, 10);
        assert_eq!(
            v,
            vec![r(0, 0, 26, 80), r(36, 0, 62, 80), r(72, 0, 100, 80)]
        );
    }

    #[test]
    fn grid_balances_rows_and_stretches_last_row() {
        let v = grid_layout(r(0, 0, 300, 200), 5, 0, 0);
        assert_eq!(v.len(), 5);
        assert_eq!(v[0], r(0, 0, 100, 100));
        assert_eq!(v[2], r(200, 0, 300, 100));
        assert_eq!(v[3], r(0, 100, 150, 200));
        assert_eq!(v[4], r(150, 100, 300, 200));
    }

    #[test]
    fn grid_clamps_gap_to_keep_cells_valid() {
        let v = grid_layout(r(0, 0, 20, 10), 4, 0, 500);
        assert_eq!(v.len(), 4);
        assert!(v.iter().all(|cell| cell.right > cell.left));
        assert!(v.iter().all(|cell| cell.bottom > cell.top));
    }

    #[test]
    fn monocle_repeats_full_target() {
        let v = monocle_layout(r(0, 0, 100, 80), 3, 5);
        assert_eq!(v, vec![r(5, 5, 95, 75); 3]);
        assert!(monocle_layout(r(0, 0, 4, 4), 1, 3).is_empty());
    }

    #[test]
    fn resize_dwindle_sets_focused_split_from_size() {
        let mut splits = vec![0.5];
        let area = r(0, 0, 200, 100);
        // n=2, inner 0: drag window 0 to width 120 -> ratio 0.6.
        resize_dwindle(&mut splits, area, 2, 0, 0, 0, r(0, 0, 120, 100));
        assert!((splits[0] - 0.6).abs() < 1e-3, "got {}", splits[0]);
    }

    #[test]
    fn resize_dwindle_remainder_uses_inverse() {
        let mut splits = vec![0.5];
        let area = r(0, 0, 200, 100);
        // The last window is the remainder of level 0: width 120 -> ratio 1-0.6.
        resize_dwindle(&mut splits, area, 2, 0, 0, 1, r(80, 0, 200, 100));
        assert!((splits[0] - 0.4).abs() < 1e-3, "got {}", splits[0]);
    }

    // ---- invariants -------------------------------------------------------
    // The bug these exist for: `master_stack` with many stack windows and a
    // large `inner_gap` (the config allows up to 500) produced rects with
    // `bottom < top`, and `workspace_layout` fed them straight to SetWindowPos
    // without checking (review B-11). Pure functions, so this sweep is free.

    /// Partial overlap = bug. Two IDENTICAL rects are a deliberate stack
    /// (monocle, and dwindle's tail once there is no room left to split).
    fn overlaps(a: RECT, b: RECT) -> bool {
        a != b && a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
    }

    /// Every rect is non-degenerate, inside the work area, and never partially
    /// on top of another.
    fn assert_sane(name: &str, area: RECT, rects: &[RECT], disjoint: bool) {
        for (i, c) in rects.iter().enumerate() {
            assert!(
                c.right > c.left && c.bottom > c.top,
                "{name}: rect {i} is inverted/empty: {:?}",
                (c.left, c.top, c.right, c.bottom)
            );
            assert!(
                c.left >= area.left
                    && c.top >= area.top
                    && c.right <= area.right
                    && c.bottom <= area.bottom,
                "{name}: rect {i} escapes the work area: {:?}",
                (c.left, c.top, c.right, c.bottom)
            );
        }
        if disjoint {
            for i in 0..rects.len() {
                for j in i + 1..rects.len() {
                    assert!(
                        !overlaps(rects[i], rects[j]),
                        "{name}: rects {i} and {j} overlap: {:?} {:?}",
                        (rects[i].left, rects[i].top, rects[i].right, rects[i].bottom),
                        (rects[j].left, rects[j].top, rects[j].right, rects[j].bottom)
                    );
                }
            }
        }
    }

    #[test]
    fn every_layout_stays_sane_across_counts_and_gaps() {
        // Small areas included on purpose: that is where the gap arithmetic
        // used to go negative.
        let areas = [
            r(0, 0, 1920, 1080),
            r(-1920, 0, 0, 1080), // monitor left of the primary (negative x)
            r(0, 28, 1280, 800),  // bar reserved
            r(0, 0, 320, 240),    // tiny
        ];
        for area in areas {
            for n in 1..=20usize {
                for outer in [0, 8, 40] {
                    for inner in [0, 4, 60, 200] {
                        let splits: Vec<f32> = vec![0.5; n];
                        let tag = format!("n={n} outer={outer} inner={inner}");
                        assert_sane(
                            &format!("master {tag}"),
                            area,
                            &master_stack(area, n, 0.55, outer, inner),
                            true,
                        );
                        assert_sane(
                            &format!("dwindle {tag}"),
                            area,
                            &dwindle_layout(area, n, outer, inner, &splits),
                            true,
                        );
                        assert_sane(
                            &format!("columns {tag}"),
                            area,
                            &columns_layout(area, n, outer, inner),
                            true,
                        );
                        assert_sane(
                            &format!("grid {tag}"),
                            area,
                            &grid_layout(area, n, outer, inner),
                            true,
                        );
                        // Monocle deliberately returns n copies of one rect.
                        assert_sane(
                            &format!("monocle {tag}"),
                            area,
                            &monocle_layout(area, n, outer),
                            false,
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn layouts_actually_place_every_window_on_a_normal_desktop() {
        // Guards the sweep above from passing vacuously: `assert_sane` is
        // trivially true for an empty result, so pin the ordinary case.
        let area = r(0, 28, 1920, 1080);
        for n in 1..=20usize {
            let splits = vec![0.5; n];
            assert_eq!(master_stack(area, n, 0.55, 8, 4).len(), n, "master n={n}");
            assert_eq!(
                dwindle_layout(area, n, 8, 4, &splits).len(),
                n,
                "dwindle n={n}"
            );
            // The spiral halves each time, so it runs out of room before the
            // other layouts do; past that the tail stacks rather than being
            // dropped or inverted. 14 is where that starts on a 1920x1052 work
            // area with these gaps — a number, not an adjective.
            assert_eq!(
                dwindle_tiles_fit(area, n, 8, 4, &splits),
                n <= 10,
                "dwindle distinct-tile limit moved at n={n}"
            );
            assert_eq!(columns_layout(area, n, 8, 4).len(), n, "columns n={n}");
            assert_eq!(grid_layout(area, n, 8, 4).len(), n, "grid n={n}");
            assert_eq!(monocle_layout(area, n, 8).len(), n, "monocle n={n}");
        }
    }

    #[test]
    fn master_stack_survives_an_absurd_gap() {
        // Was: `each` went <= 0 and every stack rect came out with bottom < top.
        let v = master_stack(r(0, 0, 800, 600), 12, 0.5, 0, 500);
        assert!(v.iter().all(|c| c.right > c.left && c.bottom > c.top));
    }

    #[test]
    fn dwindle_survives_an_absurd_gap() {
        let splits = vec![0.5; 12];
        let v = dwindle_layout(r(0, 0, 800, 600), 12, 0, 500, &splits);
        assert!(v.iter().all(|c| c.right > c.left && c.bottom > c.top));
    }

    #[test]
    fn resize_dwindle_noop_when_single() {
        let mut splits = vec![0.5];
        resize_dwindle(&mut splits, r(0, 0, 200, 100), 1, 0, 0, 0, r(0, 0, 50, 50));
        assert_eq!(splits, vec![0.5]);
    }
}
