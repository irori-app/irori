//! Squarified treemap layout (Bruls, Huizing and van Wijk): rectangles whose
//! areas follow the values, kept as close to square as the values allow, so
//! the largest games read at a glance and small ones still get a tile.
//!
//! Pure geometry in fractions of the bounds, so the view lays tiles out with
//! relative lengths and the layout is unit-tested.

/// A rectangle in fractions of the treemap's width and height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Lay out `values` (largest first gives the best result) in a `width` by
/// `height` area, returning one rectangle per value in fractions of that area,
/// in the same order. Values of zero or less get an empty rectangle.
pub(crate) fn squarify(values: &[f64], width: f64, height: f64) -> Vec<Rect> {
    let mut out = vec![
        Rect {
            x: 0.,
            y: 0.,
            w: 0.,
            h: 0.,
        };
        values.len()
    ];
    let total: f64 = values.iter().filter(|v| **v > 0.).sum();
    if total <= 0. || width <= 0. || height <= 0. {
        return out;
    }
    let scale = width * height / total;
    let items: Vec<(usize, f64)> = values
        .iter()
        .enumerate()
        .filter(|(_, v)| **v > 0.)
        .map(|(ix, v)| (ix, v * scale))
        .collect();

    // The part still free, in the same units as the areas.
    let (mut x, mut y, mut w, mut h) = (0., 0., width, height);
    let mut row: Vec<(usize, f64)> = Vec::new();
    let mut rest = items.as_slice();
    while let Some((&next, tail)) = rest.split_first() {
        let side = w.min(h);
        let mut with = row.clone();
        with.push(next);
        if row.is_empty() || worst(&with, side) <= worst(&row, side) {
            row = with;
            rest = tail;
            continue;
        }
        place(&row, &mut (x, y, w, h), &mut out, width, height);
        (x, y, w, h) = free_after(&row, (x, y, w, h));
        row.clear();
    }
    if !row.is_empty() {
        place(&row, &mut (x, y, w, h), &mut out, width, height);
    }
    out
}

/// The worst aspect ratio of `row` laid along a side of length `side`.
fn worst(row: &[(usize, f64)], side: f64) -> f64 {
    let sum: f64 = row.iter().map(|(_, a)| a).sum();
    let (min, max) = row.iter().fold((f64::INFINITY, 0f64), |(lo, hi), (_, a)| {
        (lo.min(*a), hi.max(*a))
    });
    let side2 = side * side;
    let sum2 = sum * sum;
    (side2 * max / sum2).max(sum2 / (side2 * min))
}

/// Lay `row` along the shorter side of the free part.
fn place(
    row: &[(usize, f64)],
    free: &mut (f64, f64, f64, f64),
    out: &mut [Rect],
    width: f64,
    height: f64,
) {
    let (x, y, w, h) = *free;
    let sum: f64 = row.iter().map(|(_, a)| a).sum();
    let mut offset = 0.;
    for (ix, area) in row {
        let rect = if w >= h {
            // A column on the left, as wide as the row's area needs.
            let column = sum / h;
            let length = area / column;
            let r = (x, y + offset, column, length);
            offset += length;
            r
        } else {
            // A row along the top.
            let band = sum / w;
            let length = area / band;
            let r = (x + offset, y, length, band);
            offset += length;
            r
        };
        out[*ix] = Rect {
            x: (rect.0 / width) as f32,
            y: (rect.1 / height) as f32,
            w: (rect.2 / width) as f32,
            h: (rect.3 / height) as f32,
        };
    }
}

/// The free part once `row` took its strip.
fn free_after(row: &[(usize, f64)], (x, y, w, h): (f64, f64, f64, f64)) -> (f64, f64, f64, f64) {
    let sum: f64 = row.iter().map(|(_, a)| a).sum();
    if w >= h {
        let column = sum / h;
        (x + column, y, (w - column).max(0.), h)
    } else {
        let band = sum / w;
        (x, y + band, w, (h - band).max(0.))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    fn overlaps(a: &Rect, b: &Rect) -> bool {
        a.x + EPS < b.x + b.w
            && b.x + EPS < a.x + a.w
            && a.y + EPS < b.y + b.h
            && b.y + EPS < a.y + a.h
    }

    fn check(values: &[f64], width: f64, height: f64) -> Vec<Rect> {
        let rects = squarify(values, width, height);
        assert_eq!(rects.len(), values.len());
        let total: f64 = values.iter().filter(|v| **v > 0.).sum();
        let mut covered = 0.;
        for (rect, value) in rects.iter().zip(values) {
            assert!(rect.x >= -EPS && rect.y >= -EPS, "{rect:?}");
            assert!(
                rect.x + rect.w <= 1. + EPS && rect.y + rect.h <= 1. + EPS,
                "{rect:?}"
            );
            let share = (*value).max(0.) / total;
            assert!(
                (rect.w * rect.h - share as f32).abs() < 1e-3,
                "{rect:?} should cover {share}"
            );
            covered += rect.w * rect.h;
        }
        assert!((covered - 1.).abs() < 1e-3, "covers {covered}");
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                assert!(!overlaps(a, b), "{a:?} overlaps {b:?}");
            }
        }
        rects
    }

    #[test]
    fn areas_follow_the_values_and_fill_the_bounds() {
        check(&[6., 6., 4., 3., 2., 2., 1.], 6., 4.);
        check(&[120., 80., 45., 30., 12., 5., 2., 1., 0.5], 16., 9.);
        check(&[1.], 3., 1.);
    }

    #[test]
    fn equal_values_in_a_square_make_a_grid() {
        let rects = check(&[1., 1., 1., 1.], 1., 1.);
        for r in rects {
            assert!((r.w - 0.5).abs() < EPS && (r.h - 0.5).abs() < EPS, "{r:?}");
        }
    }

    #[test]
    fn tiles_stay_close_to_square() {
        let values: Vec<f64> = (1..=20).rev().map(f64::from).collect();
        let rects = check(&values, 16., 9.);
        for r in &rects {
            let (w, h) = (r.w * 16., r.h * 9.);
            assert!(w.max(h) / w.min(h) < 4.5, "{r:?}");
        }
    }

    #[test]
    fn empty_values_get_empty_rectangles() {
        let rects = squarify(&[0., 5., -1.], 2., 1.);
        assert_eq!(rects[0].w * rects[0].h, 0.);
        assert_eq!(rects[2].w * rects[2].h, 0.);
        assert!((rects[1].w - 1.).abs() < EPS && (rects[1].h - 1.).abs() < EPS);
        assert!(squarify(&[], 1., 1.).is_empty());
        assert!(squarify(&[0.], 1., 1.)[0].w == 0.);
    }
}
