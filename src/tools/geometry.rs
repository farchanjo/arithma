//! Geometry — areas, volumes, distances for common shapes.
//!
//! Formulas use `f64` since the dominant constant is π. For the exact-precision
//! variants, callers can compose `evaluate_exact` with the `pi` constant.

use std::f64::consts::PI;

use crate::mcp::message::{ErrorCode, Response, error, error_with_detail};
use crate::tools::numeric::snap_near_integer;

const TOOL_CIRCLE_AREA: &str = "CIRCLE_AREA";
const TOOL_CIRCLE_PERIMETER: &str = "CIRCLE_PERIMETER";
const TOOL_SPHERE_VOLUME: &str = "SPHERE_VOLUME";
const TOOL_SPHERE_AREA: &str = "SPHERE_AREA";
const TOOL_TRIANGLE_AREA: &str = "TRIANGLE_AREA";
const TOOL_POLYGON_AREA: &str = "POLYGON_AREA";
const TOOL_CONE_VOLUME: &str = "CONE_VOLUME";
const TOOL_CYLINDER_VOLUME: &str = "CYLINDER_VOLUME";
const TOOL_DISTANCE_2D: &str = "DISTANCE_2D";
const TOOL_DISTANCE_3D: &str = "DISTANCE_3D";
const TOOL_REGULAR_POLYGON: &str = "REGULAR_POLYGON";
const TOOL_POINT_TO_LINE: &str = "POINT_TO_LINE_DISTANCE";

fn parse_f64(tool: &str, label: &str, value: &str) -> Result<f64, String> {
    value.trim().parse::<f64>().map_err(|_| {
        error_with_detail(
            tool,
            ErrorCode::ParseError,
            "value is not a valid number",
            &format!("{label}={value}"),
        )
    })
}

fn parse_csv(tool: &str, label: &str, input: &str) -> Result<Vec<f64>, String> {
    let mut out = Vec::new();
    for part in input.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        match trimmed.parse::<f64>() {
            Ok(v) if v.is_finite() => out.push(v),
            _ => {
                return Err(error_with_detail(
                    tool,
                    ErrorCode::ParseError,
                    "list element is not a finite number",
                    &format!("{label}={trimmed}"),
                ));
            }
        }
    }
    Ok(out)
}

/// Accept any finite non-negative length — radius/height/side of 0 describes
/// the degenerate figure whose area/volume/perimeter collapses to 0. Previously
/// this rejected 0 with `DOMAIN_ERROR`, which surprised callers who expected
/// `circleArea(0) → 0`.
fn require_non_negative(tool: &str, label: &str, value: f64) -> Result<f64, String> {
    if value >= 0.0 && value.is_finite() {
        Ok(value)
    } else {
        Err(error_with_detail(
            tool,
            ErrorCode::DomainError,
            "value must be a non-negative finite number",
            &format!("{label}={value}"),
        ))
    }
}

fn format_f64(value: f64) -> String {
    format!("{value:?}")
}

/// Build an inline `RESULT` response, but reject silent IEEE ±∞/NaN. Geometry
/// formulas cascade through `π * r²` and similar products that overflow to
/// inf once `r > ~1e154`; callers want an OVERFLOW envelope, not `inf`.
fn finite_result(tool: &str, value: f64) -> String {
    if value.is_finite() {
        Response::ok(tool).result(format_f64(value)).build()
    } else {
        error_with_detail(
            tool,
            ErrorCode::Overflow,
            "result exceeds the f64 finite range",
            &format!("result={value:?}"),
        )
    }
}

#[must_use]
pub fn circle_area(radius: &str) -> String {
    let r = match parse_f64(TOOL_CIRCLE_AREA, "radius", radius)
        .and_then(|v| require_non_negative(TOOL_CIRCLE_AREA, "radius", v))
    {
        Ok(v) => v,
        Err(e) => return e,
    };
    finite_result(TOOL_CIRCLE_AREA, PI * r * r)
}

#[must_use]
pub fn circle_perimeter(radius: &str) -> String {
    let r = match parse_f64(TOOL_CIRCLE_PERIMETER, "radius", radius)
        .and_then(|v| require_non_negative(TOOL_CIRCLE_PERIMETER, "radius", v))
    {
        Ok(v) => v,
        Err(e) => return e,
    };
    finite_result(TOOL_CIRCLE_PERIMETER, 2.0 * PI * r)
}

#[must_use]
pub fn sphere_volume(radius: &str) -> String {
    let r = match parse_f64(TOOL_SPHERE_VOLUME, "radius", radius)
        .and_then(|v| require_non_negative(TOOL_SPHERE_VOLUME, "radius", v))
    {
        Ok(v) => v,
        Err(e) => return e,
    };
    finite_result(TOOL_SPHERE_VOLUME, 4.0 / 3.0 * PI * r.powi(3))
}

#[must_use]
pub fn sphere_area(radius: &str) -> String {
    let r = match parse_f64(TOOL_SPHERE_AREA, "radius", radius)
        .and_then(|v| require_non_negative(TOOL_SPHERE_AREA, "radius", v))
    {
        Ok(v) => v,
        Err(e) => return e,
    };
    finite_result(TOOL_SPHERE_AREA, 4.0 * PI * r * r)
}

/// Triangle area via Heron's formula. `sides` is "a,b,c".
#[must_use]
pub fn triangle_area(sides: &str) -> String {
    let arr = match parse_csv(TOOL_TRIANGLE_AREA, "sides", sides) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if arr.len() != 3 {
        return error_with_detail(
            TOOL_TRIANGLE_AREA,
            ErrorCode::InvalidInput,
            "expected exactly 3 sides",
            &format!("got={}", arr.len()),
        );
    }
    let (a, b, c) = (arr[0], arr[1], arr[2]);
    if a <= 0.0 || b <= 0.0 || c <= 0.0 {
        return error(
            TOOL_TRIANGLE_AREA,
            ErrorCode::DomainError,
            "all sides must be positive",
        );
    }
    if a + b <= c || a + c <= b || b + c <= a {
        return error(
            TOOL_TRIANGLE_AREA,
            ErrorCode::DomainError,
            "triangle inequality violated",
        );
    }
    let s = (a + b + c) / 2.0;
    let area = (s * (s - a) * (s - b) * (s - c)).sqrt();
    finite_result(TOOL_TRIANGLE_AREA, area)
}

/// Polygon area via the Shoelace formula. `coordinates` is
/// "x1,y1,x2,y2,...,xn,yn" (vertices in order, polygon closes implicitly).
#[must_use]
pub fn polygon_area(coordinates: &str) -> String {
    let arr = match parse_csv(TOOL_POLYGON_AREA, "coordinates", coordinates) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if arr.len() < 6 || arr.len() % 2 != 0 {
        return error_with_detail(
            TOOL_POLYGON_AREA,
            ErrorCode::InvalidInput,
            "expected an even number of values, at least 6 (3 vertices)",
            &format!("count={}", arr.len()),
        );
    }
    let n = arr.len() / 2;
    let mut sum = 0.0;
    for i in 0..n {
        let x_i = arr[2 * i];
        let y_i = arr[2 * i + 1];
        let j = (i + 1) % n;
        let x_j = arr[2 * j];
        let y_j = arr[2 * j + 1];
        sum += x_i.mul_add(y_j, -(x_j * y_i));
    }
    let area = sum.abs() / 2.0;
    if !area.is_finite() {
        return error_with_detail(
            TOOL_POLYGON_AREA,
            ErrorCode::Overflow,
            "result exceeds the f64 finite range",
            &format!("area={area:?}"),
        );
    }
    Response::ok(TOOL_POLYGON_AREA)
        .field("AREA", format_f64(area))
        .field("VERTICES", n.to_string())
        .build()
}

#[must_use]
pub fn cone_volume(radius: &str, height: &str) -> String {
    let r = match parse_f64(TOOL_CONE_VOLUME, "radius", radius)
        .and_then(|v| require_non_negative(TOOL_CONE_VOLUME, "radius", v))
    {
        Ok(v) => v,
        Err(e) => return e,
    };
    let h = match parse_f64(TOOL_CONE_VOLUME, "height", height)
        .and_then(|v| require_non_negative(TOOL_CONE_VOLUME, "height", v))
    {
        Ok(v) => v,
        Err(e) => return e,
    };
    finite_result(TOOL_CONE_VOLUME, PI * r * r * h / 3.0)
}

#[must_use]
pub fn cylinder_volume(radius: &str, height: &str) -> String {
    let r = match parse_f64(TOOL_CYLINDER_VOLUME, "radius", radius)
        .and_then(|v| require_non_negative(TOOL_CYLINDER_VOLUME, "radius", v))
    {
        Ok(v) => v,
        Err(e) => return e,
    };
    let h = match parse_f64(TOOL_CYLINDER_VOLUME, "height", height)
        .and_then(|v| require_non_negative(TOOL_CYLINDER_VOLUME, "height", v))
    {
        Ok(v) => v,
        Err(e) => return e,
    };
    finite_result(TOOL_CYLINDER_VOLUME, PI * r * r * h)
}

#[must_use]
pub fn distance_2d(p1: &str, p2: &str) -> String {
    let a = match parse_csv(TOOL_DISTANCE_2D, "p1", p1) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let b = match parse_csv(TOOL_DISTANCE_2D, "p2", p2) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if a.len() != 2 || b.len() != 2 {
        return error(
            TOOL_DISTANCE_2D,
            ErrorCode::InvalidInput,
            "p1 and p2 must each have exactly 2 coordinates (x,y)",
        );
    }
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    finite_result(TOOL_DISTANCE_2D, dx.hypot(dy))
}

#[must_use]
pub fn distance_3d(p1: &str, p2: &str) -> String {
    let a = match parse_csv(TOOL_DISTANCE_3D, "p1", p1) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let b = match parse_csv(TOOL_DISTANCE_3D, "p2", p2) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if a.len() != 3 || b.len() != 3 {
        return error(
            TOOL_DISTANCE_3D,
            ErrorCode::InvalidInput,
            "p1 and p2 must each have exactly 3 coordinates (x,y,z)",
        );
    }
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    // Scale-protected: factor the largest component out before squaring so
    // `(1e300, 1e300, 1e300)` distance stays in f64 instead of saturating
    // at `+∞` during the naive `dx² + dy² + dz²` reduction.
    let max_abs = dx.abs().max(dy.abs()).max(dz.abs());
    let distance = if max_abs == 0.0 {
        0.0
    } else {
        let (nx, ny, nz) = (dx / max_abs, dy / max_abs, dz / max_abs);
        let sum_sq = nx.mul_add(nx, ny.mul_add(ny, nz * nz));
        max_abs * sum_sq.sqrt()
    };
    finite_result(TOOL_DISTANCE_3D, distance)
}

/// Regular polygon properties from sides count and side length.
/// Returns area, perimeter, apothem, and circumradius.
#[must_use]
pub fn regular_polygon(sides: i32, side_length: &str) -> String {
    if sides < 3 {
        return error_with_detail(
            TOOL_REGULAR_POLYGON,
            ErrorCode::InvalidInput,
            "sides must be at least 3",
            &format!("sides={sides}"),
        );
    }
    let s = match parse_f64(TOOL_REGULAR_POLYGON, "sideLength", side_length)
        .and_then(|v| require_non_negative(TOOL_REGULAR_POLYGON, "sideLength", v))
    {
        Ok(v) => v,
        Err(e) => return e,
    };
    let n = f64::from(sides);
    let perimeter = n * s;
    // PI and .tan()/.sin() drop ~1 ULP into the result; snap values within
    // 1e-9 of an integer so `regular_polygon(6, 1)` reports a circumradius of
    // `1.0` instead of the literal `1.0000000000000002`.
    let apothem = snap_near_integer(s / (2.0 * (PI / n).tan()));
    let circumradius = snap_near_integer(s / (2.0 * (PI / n).sin()));
    let area = snap_near_integer(perimeter * apothem / 2.0);
    Response::ok(TOOL_REGULAR_POLYGON)
        .field("AREA", format_f64(area))
        .field("PERIMETER", format_f64(perimeter))
        .field("APOTHEM", format_f64(apothem))
        .field("CIRCUMRADIUS", format_f64(circumradius))
        .build()
}

/// Distance from `point` to the line through `lineP1` and `lineP2`.
#[must_use]
pub fn point_to_line_distance(point: &str, line_p1: &str, line_p2: &str) -> String {
    let p = match parse_csv(TOOL_POINT_TO_LINE, "point", point) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let a = match parse_csv(TOOL_POINT_TO_LINE, "lineP1", line_p1) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let b = match parse_csv(TOOL_POINT_TO_LINE, "lineP2", line_p2) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if p.len() != 2 || a.len() != 2 || b.len() != 2 {
        return error(
            TOOL_POINT_TO_LINE,
            ErrorCode::InvalidInput,
            "all three inputs must be 2D points (x,y)",
        );
    }
    let bx_ax = b[0] - a[0];
    let by_ay = b[1] - a[1];
    let num = bx_ax.mul_add(a[1] - p[1], -((a[0] - p[0]) * by_ay)).abs();
    let den = bx_ax.hypot(by_ay);
    if den == 0.0 {
        return error(
            TOOL_POINT_TO_LINE,
            ErrorCode::DomainError,
            "lineP1 and lineP2 are coincident — line is undefined",
        );
    }
    finite_result(TOOL_POINT_TO_LINE, num / den)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_field(out: &str, key: &str, expected: f64) {
        // Match `<sep>KEY: value` where sep is the inline " | " or the trailing
        // ": " right after the tool header. This avoids matching `KEY` as a
        // substring of the tool name (e.g. POLYGON_AREA contains AREA).
        let primary = format!(" | {key}: ");
        let header = format!(": OK | {key}: ");
        let part = out
            .split(&primary)
            .nth(1)
            .or_else(|| out.split(&header).nth(1))
            .unwrap_or_else(|| panic!("field {key} not found in `{out}`"));
        let value_str: String = part
            .chars()
            .take_while(|c| *c != ' ' && *c != '\n')
            .collect();
        let v: f64 = value_str
            .parse()
            .unwrap_or_else(|e| panic!("parse {value_str:?} for {key}: {e} (full: `{out}`)"));
        assert!(
            (v - expected).abs() < 1e-6,
            "{key}: expected ~{expected}, got {v} in `{out}`"
        );
    }

    #[test]
    fn circle_area_unit_radius() {
        approx_field(&circle_area("1"), "RESULT", PI);
    }

    #[test]
    fn circle_area_radius_two() {
        approx_field(&circle_area("2"), "RESULT", 4.0 * PI);
    }

    #[test]
    fn circle_perimeter_unit_radius() {
        approx_field(&circle_perimeter("1"), "RESULT", 2.0 * PI);
    }

    #[test]
    fn sphere_volume_radius_one() {
        approx_field(&sphere_volume("1"), "RESULT", 4.0 / 3.0 * PI);
    }

    #[test]
    fn sphere_area_radius_one() {
        approx_field(&sphere_area("1"), "RESULT", 4.0 * PI);
    }

    #[test]
    fn triangle_area_3_4_5_right_triangle() {
        approx_field(&triangle_area("3,4,5"), "RESULT", 6.0);
    }

    #[test]
    fn triangle_area_equilateral() {
        // Equilateral side=2 → area = sqrt(3) ≈ 1.7320508
        let out = triangle_area("2,2,2");
        approx_field(&out, "RESULT", 3.0_f64.sqrt());
    }

    #[test]
    fn triangle_area_inequality_violated() {
        let out = triangle_area("1,1,5");
        assert!(out.starts_with("TRIANGLE_AREA: ERROR"));
    }

    #[test]
    fn circle_area_zero_radius_is_zero() {
        // Degenerate circle: radius 0 ⇒ area 0, not a DOMAIN_ERROR.
        approx_field(&circle_area("0"), "RESULT", 0.0);
        approx_field(&sphere_volume("0"), "RESULT", 0.0);
        approx_field(&sphere_area("0"), "RESULT", 0.0);
        approx_field(&circle_perimeter("0"), "RESULT", 0.0);
    }

    #[test]
    fn circle_area_negative_radius_still_rejected() {
        assert!(circle_area("-1").starts_with("CIRCLE_AREA: ERROR"));
    }

    #[test]
    fn polygon_area_unit_square() {
        // Vertices (0,0)(1,0)(1,1)(0,1) → area = 1
        let out = polygon_area("0,0,1,0,1,1,0,1");
        approx_field(&out, "AREA", 1.0);
        assert!(out.contains("VERTICES: 4"));
    }

    #[test]
    fn polygon_area_rejects_too_few_points() {
        let out = polygon_area("0,0,1,1");
        assert!(out.starts_with("POLYGON_AREA: ERROR"));
    }

    #[test]
    fn cone_volume_unit() {
        // r=1, h=3 → V = π * 1 * 3 / 3 = π
        approx_field(&cone_volume("1", "3"), "RESULT", PI);
    }

    #[test]
    fn cylinder_volume_unit() {
        // r=1, h=1 → V = π
        approx_field(&cylinder_volume("1", "1"), "RESULT", PI);
    }

    #[test]
    fn distance_2d_pythagorean() {
        approx_field(&distance_2d("0,0", "3,4"), "RESULT", 5.0);
    }

    #[test]
    fn distance_3d_unit_diagonal() {
        // (0,0,0) to (1,1,1) = sqrt(3)
        approx_field(&distance_3d("0,0,0", "1,1,1"), "RESULT", 3.0_f64.sqrt());
    }

    #[test]
    fn regular_polygon_square_side_2() {
        // Square (n=4, side=2): area=4, perimeter=8, apothem=1, circumradius=sqrt(2)
        let out = regular_polygon(4, "2");
        approx_field(&out, "AREA", 4.0);
        approx_field(&out, "PERIMETER", 8.0);
        approx_field(&out, "APOTHEM", 1.0);
        approx_field(&out, "CIRCUMRADIUS", 2.0_f64.sqrt());
    }

    #[test]
    fn regular_polygon_hexagon_side_1() {
        // Hexagon area = 3*sqrt(3)/2 ≈ 2.598. Circumradius = side = 1 exactly.
        let out = regular_polygon(6, "1");
        approx_field(&out, "AREA", 3.0 * 3.0_f64.sqrt() / 2.0);
        // Regression: the 2π/sin round-trip leaked ~1 ULP and printed
        // `CIRCUMRADIUS: 1.0000000000000002`.
        assert!(out.contains("CIRCUMRADIUS: 1.0"), "got: {out}");
        assert!(!out.contains("1.0000000000000002"), "got: {out}");
    }

    #[test]
    fn regular_polygon_tiny_side_preserves_magnitude() {
        // Regression: `regular_polygon(3, 1e-10)` has
        // `apothem = 1e-10 / (2·tan(60°)) ≈ 2.88e-11` — a legitimate tiny
        // result that the previous `snap_near_integer` (which snapped any
        // value below `1e-9`) wrongly collapsed to `0`. The zero-target
        // guard now preserves the magnitude.
        let out = regular_polygon(3, "1e-10");
        assert!(out.contains("PERIMETER: 3e-10"), "got {out}");
        assert!(!out.contains("APOTHEM: 0.0"), "got {out}");
        assert!(!out.contains("CIRCUMRADIUS: 0.0"), "got {out}");
        assert!(out.contains("e-11"), "got {out}");
    }

    #[test]
    fn regular_polygon_min_sides_enforced() {
        let out = regular_polygon(2, "1");
        assert!(out.starts_with("REGULAR_POLYGON: ERROR"));
    }

    #[test]
    fn point_to_line_distance_basic() {
        // Point (0,0) to line through (1,0)-(1,5) is exactly 1
        approx_field(&point_to_line_distance("0,0", "1,0", "1,5"), "RESULT", 1.0);
    }

    #[test]
    fn point_to_line_coincident_endpoints_errors() {
        let out = point_to_line_distance("0,0", "1,1", "1,1");
        assert!(out.starts_with("POINT_TO_LINE_DISTANCE: ERROR"));
    }
}
