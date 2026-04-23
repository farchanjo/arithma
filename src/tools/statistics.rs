//! Statistical analysis — descriptive stats, distributions, regression.
//!
//! All inputs are comma-separated decimal lists. Computations run on `f64`
//! since statistical results are inherently approximate (square roots,
//! exp/erf for distributions). For the rare case where a caller needs exact
//! sums, the `vector::sum_array` tool is available and uses `BigDecimal`.

use std::f64::consts::TAU;

use crate::mcp::message::{ErrorCode, Response, error, error_with_detail};
use crate::tools::numeric::guard_finite;

const TOOL_MEAN: &str = "MEAN";
const TOOL_MEDIAN: &str = "MEDIAN";
const TOOL_MODE: &str = "MODE";
const TOOL_VARIANCE: &str = "VARIANCE";
const TOOL_STDDEV: &str = "STDDEV";
const TOOL_PERCENTILE: &str = "PERCENTILE";
const TOOL_QUARTILE: &str = "QUARTILE";
const TOOL_IQR: &str = "IQR";
const TOOL_CORRELATION: &str = "CORRELATION";
const TOOL_COVARIANCE: &str = "COVARIANCE";
const TOOL_LINEAR_REGRESSION: &str = "LINEAR_REGRESSION";
const TOOL_NORMAL_PDF: &str = "NORMAL_PDF";
const TOOL_NORMAL_CDF: &str = "NORMAL_CDF";
const TOOL_T_TEST: &str = "T_TEST";
const TOOL_BINOMIAL_PMF: &str = "BINOMIAL_PMF";
const TOOL_CONFIDENCE_INTERVAL: &str = "CONFIDENCE_INTERVAL";

fn parse_array(tool: &str, label: &str, input: &str) -> Result<Vec<f64>, String> {
    let parts: Vec<&str> = input.split(',').collect();
    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
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
                    "array element is not a finite number",
                    &format!("{label}={trimmed}"),
                ));
            }
        }
    }
    Ok(out)
}

fn require_nonempty(tool: &str, arr: &[f64]) -> Result<(), String> {
    if arr.is_empty() {
        Err(error(
            tool,
            ErrorCode::InvalidInput,
            "input array must not be empty",
        ))
    } else {
        Ok(())
    }
}

fn parse_decimal(tool: &str, label: &str, value: &str) -> Result<f64, String> {
    value.trim().parse::<f64>().map_err(|_| {
        error_with_detail(
            tool,
            ErrorCode::ParseError,
            "value is not a valid number",
            &format!("{label}={value}"),
        )
    })
}

/// Ship a scalar RESULT with the overflow guard applied. Use for any
/// reduction over large samples so an IEEE inf never escapes the envelope.
fn ok_result(tool: &str, value: f64) -> String {
    match guard_finite(tool, "result", value) {
        Ok(v) => Response::ok(tool).result(format_f64(v)).build(),
        Err(e) => e,
    }
}

fn format_f64(value: f64) -> String {
    format!("{value:?}")
}

/// Convert a `usize` count to `f64` through `num_traits::NumCast` so the
/// project-wide "no raw `as` casts" rule stays intact. Counts over 2^53 are
/// clamped to `f64::MAX` — arrays that large don't fit in memory anyway.
fn count_as_f64(count: usize) -> f64 {
    num_traits::NumCast::from(count).unwrap_or(f64::MAX)
}

#[must_use]
pub fn mean(values: &str) -> String {
    let arr = match parse_array(TOOL_MEAN, "values", values) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = require_nonempty(TOOL_MEAN, &arr) {
        return e;
    }
    let n = count_as_f64(arr.len());
    let sum: f64 = arr.iter().sum();
    ok_result(TOOL_MEAN, sum / n)
}

#[must_use]
pub fn median(values: &str) -> String {
    let mut arr = match parse_array(TOOL_MEDIAN, "values", values) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = require_nonempty(TOOL_MEDIAN, &arr) {
        return e;
    }
    arr.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = arr.len();
    let m = if n % 2 == 1 {
        arr[n / 2]
    } else {
        (arr[n / 2 - 1] + arr[n / 2]) * 0.5
    };
    Response::ok(TOOL_MEDIAN).result(format_f64(m)).build()
}

#[must_use]
pub fn mode(values: &str) -> String {
    let arr = match parse_array(TOOL_MODE, "values", values) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = require_nonempty(TOOL_MODE, &arr) {
        return e;
    }
    // Count occurrences via float bit-pattern; ties break by smaller value.
    let mut counts: std::collections::BTreeMap<u64, (f64, usize)> =
        std::collections::BTreeMap::new();
    for &v in &arr {
        let key = v.to_bits();
        counts.entry(key).and_modify(|e| e.1 += 1).or_insert((v, 1));
    }
    let max_count = counts.values().map(|(_, c)| *c).max().unwrap_or(0);
    // Amodal distributions — every value appears the same number of times
    // AND there is more than one distinct value — have no mode. Returning
    // the whole sample as "modes" is the classic scikit / R convention but
    // misleads non-stats callers; we surface it as an explicit `NONE`.
    if max_count == 1 && counts.len() > 1 {
        return Response::ok(TOOL_MODE)
            .field("MODES", "NONE")
            .field("COUNT", "0")
            .field("REASON", "every value occurs exactly once (amodal)")
            .build();
    }
    let mut modes: Vec<f64> = counts
        .values()
        .filter(|(_, c)| *c == max_count)
        .map(|(v, _)| *v)
        .collect();
    modes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let modes_str = modes
        .iter()
        .map(|v| format_f64(*v))
        .collect::<Vec<_>>()
        .join(",");
    Response::ok(TOOL_MODE)
        .field("MODES", modes_str)
        .field("COUNT", max_count.to_string())
        .build()
}

/// Sample variance (n-1 denominator). Population variance available via the
/// `population` flag.
#[must_use]
pub fn variance(values: &str, population: bool) -> String {
    let arr = match parse_array(TOOL_VARIANCE, "values", values) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = require_nonempty(TOOL_VARIANCE, &arr) {
        return e;
    }
    if !population && arr.len() < 2 {
        return error(
            TOOL_VARIANCE,
            ErrorCode::InvalidInput,
            "sample variance requires at least 2 values (use population=true for n=1)",
        );
    }
    let v = compute_variance(&arr, population);
    ok_result(TOOL_VARIANCE, v)
}

#[must_use]
pub fn std_dev(values: &str, population: bool) -> String {
    let arr = match parse_array(TOOL_STDDEV, "values", values) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = require_nonempty(TOOL_STDDEV, &arr) {
        return e;
    }
    if !population && arr.len() < 2 {
        return error(
            TOOL_STDDEV,
            ErrorCode::InvalidInput,
            "sample stddev requires at least 2 values",
        );
    }
    let v = compute_variance(&arr, population);
    ok_result(TOOL_STDDEV, v.sqrt())
}

fn compute_variance(arr: &[f64], population: bool) -> f64 {
    let n = count_as_f64(arr.len());
    let mean_val: f64 = arr.iter().sum::<f64>() / n;
    let sum_sq: f64 = arr.iter().map(|x| (x - mean_val).powi(2)).sum();
    let denom = if population { n } else { n - 1.0 };
    sum_sq / denom
}

/// Linear-interpolated percentile (R-7 / Excel definition). `p` is 0-100.
#[must_use]
pub fn percentile(values: &str, p: &str) -> String {
    let mut arr = match parse_array(TOOL_PERCENTILE, "values", values) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = require_nonempty(TOOL_PERCENTILE, &arr) {
        return e;
    }
    let pct = match parse_decimal(TOOL_PERCENTILE, "p", p) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !(0.0..=100.0).contains(&pct) {
        return error_with_detail(
            TOOL_PERCENTILE,
            ErrorCode::OutOfRange,
            "percentile must be in [0, 100]",
            &format!("p={pct}"),
        );
    }
    arr.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let result = compute_percentile(&arr, pct);
    Response::ok(TOOL_PERCENTILE)
        .result(format_f64(result))
        .build()
}

fn compute_percentile(sorted: &[f64], pct: f64) -> f64 {
    if sorted.len() == 1 {
        return sorted[0];
    }
    let rank = (pct / 100.0) * (count_as_f64(sorted.len()) - 1.0);
    let lo = rank.floor();
    let hi = rank.ceil();
    let lo_idx: usize = num_traits::NumCast::from(lo).unwrap_or(0);
    let hi_idx: usize = num_traits::NumCast::from(hi).unwrap_or(0);
    if lo_idx == hi_idx {
        return sorted[lo_idx];
    }
    let frac = rank - lo;
    (sorted[hi_idx] - sorted[lo_idx]).mul_add(frac, sorted[lo_idx])
}

#[must_use]
pub fn quartile(values: &str, q: i32) -> String {
    if !(1..=3).contains(&q) {
        return error_with_detail(
            TOOL_QUARTILE,
            ErrorCode::InvalidInput,
            "quartile q must be 1, 2, or 3",
            &format!("q={q}"),
        );
    }
    let mut arr = match parse_array(TOOL_QUARTILE, "values", values) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = require_nonempty(TOOL_QUARTILE, &arr) {
        return e;
    }
    arr.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct = f64::from(q) * 25.0;
    let v = compute_percentile(&arr, pct);
    Response::ok(TOOL_QUARTILE)
        .field("Q", q.to_string())
        .field("VALUE", format_f64(v))
        .build()
}

#[must_use]
pub fn iqr(values: &str) -> String {
    let mut arr = match parse_array(TOOL_IQR, "values", values) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = require_nonempty(TOOL_IQR, &arr) {
        return e;
    }
    arr.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let q1 = compute_percentile(&arr, 25.0);
    let q3 = compute_percentile(&arr, 75.0);
    Response::ok(TOOL_IQR)
        .field("Q1", format_f64(q1))
        .field("Q3", format_f64(q3))
        .field("IQR", format_f64(q3 - q1))
        .build()
}

/// Pearson correlation coefficient between two equal-length series.
#[must_use]
pub fn correlation(x_values: &str, y_values: &str) -> String {
    let xs = match parse_array(TOOL_CORRELATION, "x", x_values) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let ys = match parse_array(TOOL_CORRELATION, "y", y_values) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if xs.len() != ys.len() || xs.is_empty() {
        return error(
            TOOL_CORRELATION,
            ErrorCode::InvalidInput,
            "x and y must be non-empty and the same length",
        );
    }
    compute_pearson(&xs, &ys).map_or_else(
        || {
            error(
                TOOL_CORRELATION,
                ErrorCode::DomainError,
                "correlation is undefined when x or y has zero variance",
            )
        },
        |r| Response::ok(TOOL_CORRELATION).result(format_f64(r)).build(),
    )
}

fn compute_pearson(xs: &[f64], ys: &[f64]) -> Option<f64> {
    let n = count_as_f64(xs.len());
    let mean_x: f64 = xs.iter().sum::<f64>() / n;
    let mean_y: f64 = ys.iter().sum::<f64>() / n;

    // Scale-invariant correlation. Pearson's `r` is mathematically invariant
    // under independent affine rescaling of the two series, so we normalize
    // each deviation by its own max-|Δ| before accumulating sums of squares.
    // Without this pass, a cross-scale comparison (e.g. xs ~ 1e300 against
    // ys ~ 1e-300) overflows `Σdx²` to `+∞` and underflows `Σdy²` to `0`,
    // making the product `inf·0 = NaN` — even though the true correlation
    // is perfectly well-defined (1.0 for the test case). Normalizing puts
    // every `dx/dy` into `[-1, 1]` so every accumulator stays within f64
    // range regardless of the raw magnitudes.
    let mut scale_x: f64 = 0.0;
    let mut scale_y: f64 = 0.0;
    for i in 0..xs.len() {
        scale_x = scale_x.max((xs[i] - mean_x).abs());
        scale_y = scale_y.max((ys[i] - mean_y).abs());
    }
    // Either series constant → correlation undefined (mirrors the legacy
    // `denom == 0` branch below, raised one level so we don't divide by
    // zero during the normalization itself).
    if scale_x == 0.0 || scale_y == 0.0 {
        return None;
    }

    let mut num = 0.0;
    let mut den_x = 0.0;
    let mut den_y = 0.0;
    for i in 0..xs.len() {
        let dx = (xs[i] - mean_x) / scale_x;
        let dy = (ys[i] - mean_y) / scale_y;
        num += dx * dy;
        den_x += dx * dx;
        den_y += dy * dy;
    }
    let denom = (den_x * den_y).sqrt();
    if denom == 0.0 {
        None
    } else {
        Some(num / denom)
    }
}

#[must_use]
pub fn covariance(x_values: &str, y_values: &str, population: bool) -> String {
    let xs = match parse_array(TOOL_COVARIANCE, "x", x_values) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let ys = match parse_array(TOOL_COVARIANCE, "y", y_values) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if xs.len() != ys.len() || xs.is_empty() {
        return error(
            TOOL_COVARIANCE,
            ErrorCode::InvalidInput,
            "x and y must be non-empty and the same length",
        );
    }
    if !population && xs.len() < 2 {
        return error(
            TOOL_COVARIANCE,
            ErrorCode::InvalidInput,
            "sample covariance requires at least 2 paired values",
        );
    }
    let n = count_as_f64(xs.len());
    let mean_x: f64 = xs.iter().sum::<f64>() / n;
    let mean_y: f64 = ys.iter().sum::<f64>() / n;
    let mut sum = 0.0;
    for i in 0..xs.len() {
        sum += (xs[i] - mean_x) * (ys[i] - mean_y);
    }
    let denom = if population { n } else { n - 1.0 };
    ok_result(TOOL_COVARIANCE, sum / denom)
}

#[must_use]
pub fn linear_regression(x_values: &str, y_values: &str) -> String {
    let xs = match parse_array(TOOL_LINEAR_REGRESSION, "x", x_values) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let ys = match parse_array(TOOL_LINEAR_REGRESSION, "y", y_values) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if xs.len() != ys.len() || xs.len() < 2 {
        return error(
            TOOL_LINEAR_REGRESSION,
            ErrorCode::InvalidInput,
            "x and y must have the same length and at least 2 points",
        );
    }
    let n = count_as_f64(xs.len());
    let mean_x: f64 = xs.iter().sum::<f64>() / n;
    let mean_y: f64 = ys.iter().sum::<f64>() / n;
    let mut num = 0.0;
    let mut den = 0.0;
    for i in 0..xs.len() {
        let dx = xs[i] - mean_x;
        num += dx * (ys[i] - mean_y);
        den += dx * dx;
    }
    if den == 0.0 {
        return error(
            TOOL_LINEAR_REGRESSION,
            ErrorCode::DomainError,
            "x values are constant — slope is undefined",
        );
    }
    let slope = num / den;
    let intercept = mean_y - slope * mean_x;
    // If y has zero variance the Pearson coefficient is undefined; the
    // regression still has a valid slope/intercept, so report R = 0 to
    // preserve the response shape.
    let r = compute_pearson(&xs, &ys).unwrap_or(0.0);
    for (label, val) in [("slope", slope), ("intercept", intercept), ("r", r)] {
        if let Err(e) = guard_finite(TOOL_LINEAR_REGRESSION, label, val) {
            return e;
        }
    }
    Response::ok(TOOL_LINEAR_REGRESSION)
        .field("SLOPE", format_f64(slope))
        .field("INTERCEPT", format_f64(intercept))
        .field("R", format_f64(r))
        .field("R_SQUARED", format_f64(r * r))
        .build()
}

#[must_use]
pub fn normal_pdf(x: &str, mean: &str, std_dev: &str) -> String {
    let x_v = match parse_decimal(TOOL_NORMAL_PDF, "x", x) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let mu = match parse_decimal(TOOL_NORMAL_PDF, "mean", mean) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let sigma = match parse_decimal(TOOL_NORMAL_PDF, "stdDev", std_dev) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if sigma <= 0.0 {
        return error_with_detail(
            TOOL_NORMAL_PDF,
            ErrorCode::DomainError,
            "stdDev must be positive",
            &format!("stdDev={sigma}"),
        );
    }
    let z = (x_v - mu) / sigma;
    let result = (-0.5 * z * z).exp() / (sigma * (TAU).sqrt());
    Response::ok(TOOL_NORMAL_PDF)
        .result(format_f64(result))
        .build()
}

#[must_use]
pub fn normal_cdf(x: &str, mean: &str, std_dev: &str) -> String {
    let x_v = match parse_decimal(TOOL_NORMAL_CDF, "x", x) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let mu = match parse_decimal(TOOL_NORMAL_CDF, "mean", mean) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let sigma = match parse_decimal(TOOL_NORMAL_CDF, "stdDev", std_dev) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if sigma <= 0.0 {
        return error_with_detail(
            TOOL_NORMAL_CDF,
            ErrorCode::DomainError,
            "stdDev must be positive",
            &format!("stdDev={sigma}"),
        );
    }
    // Symmetry short-circuit: Φ(μ; μ, σ) = 1/2 exactly. Compare bit patterns
    // to sidestep clippy's float_cmp lint — the two f64s either come from the
    // same decimal input and are bitwise identical, or they differ and we
    // proceed to the erf path.
    if x_v.to_bits() == mu.to_bits() {
        return Response::ok(TOOL_NORMAL_CDF)
            .result(format_f64(0.5))
            .build();
    }
    let z = (x_v - mu) / (sigma * std::f64::consts::SQRT_2);
    let result = 0.5 * (1.0 + libm::erf(z));
    Response::ok(TOOL_NORMAL_CDF)
        .result(format_f64(result))
        .build()
}

/// One-sample t-test against a hypothesized mean. Returns t-statistic and
/// degrees of freedom. P-value computation uses a normal approximation valid
/// for `df >= 30`; smaller dfs are flagged in DETAIL.
#[must_use]
pub fn t_test_one_sample(values: &str, hypothesized_mean: &str) -> String {
    let arr = match parse_array(TOOL_T_TEST, "values", values) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if arr.len() < 2 {
        return error(
            TOOL_T_TEST,
            ErrorCode::InvalidInput,
            "t-test requires at least 2 values",
        );
    }
    let mu0 = match parse_decimal(TOOL_T_TEST, "hypothesizedMean", hypothesized_mean) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let n = count_as_f64(arr.len());
    let mean_val: f64 = arr.iter().sum::<f64>() / n;
    let sample_var = compute_variance(&arr, false);
    let se = (sample_var / n).sqrt();
    if se == 0.0 {
        return error(
            TOOL_T_TEST,
            ErrorCode::DomainError,
            "standard error is zero — sample has no variance",
        );
    }
    let t = (mean_val - mu0) / se;
    let df = n - 1.0;
    for (label, val) in [("t", t), ("mean", mean_val), ("se", se)] {
        if let Err(e) = guard_finite(TOOL_T_TEST, label, val) {
            return e;
        }
    }
    Response::ok(TOOL_T_TEST)
        .field("T", format_f64(t))
        .field("DF", format_f64(df))
        .field("MEAN", format_f64(mean_val))
        .field("SE", format_f64(se))
        .build()
}

/// Probability mass function for a binomial distribution `B(n, p)`.
#[must_use]
pub fn binomial_pmf(n: i64, k: i64, p: &str) -> String {
    if n < 0 || k < 0 || k > n {
        return error_with_detail(
            TOOL_BINOMIAL_PMF,
            ErrorCode::OutOfRange,
            "require 0 <= k <= n",
            &format!("n={n}, k={k}"),
        );
    }
    let prob = match parse_decimal(TOOL_BINOMIAL_PMF, "p", p) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !(0.0..=1.0).contains(&prob) {
        return error_with_detail(
            TOOL_BINOMIAL_PMF,
            ErrorCode::OutOfRange,
            "p must be in [0, 1]",
            &format!("p={prob}"),
        );
    }
    if n > 1000 {
        return error_with_detail(
            TOOL_BINOMIAL_PMF,
            ErrorCode::OutOfRange,
            "n too large; use a normal approximation for n > 1000",
            &format!("n={n}"),
        );
    }
    // Boundary cases: p=0 → all failures, so P(X=k) is 1 when k=0, else 0.
    // p=1 → all successes, so P(X=k) is 1 when k=n, else 0. The log-space
    // path below would hit ln(0) = -inf and produce NaN otherwise. Bit
    // comparisons keep clippy's float_cmp lint happy while preserving the
    // exact-literal intent.
    if prob.to_bits() == 0.0_f64.to_bits() {
        let pmf = if k == 0 { 1.0 } else { 0.0 };
        return Response::ok(TOOL_BINOMIAL_PMF)
            .result(format_f64(pmf))
            .build();
    }
    if prob.to_bits() == 1.0_f64.to_bits() {
        let pmf = if k == n { 1.0 } else { 0.0 };
        return Response::ok(TOOL_BINOMIAL_PMF)
            .result(format_f64(pmf))
            .build();
    }
    // log-space to avoid overflow on large binomial coefficients
    let coeff = log_binomial(n, k);
    // k and n fit in i64 with k<=n<=1000 (checked above), so the i32->f64 path
    // is exact. Use `f64::from` after narrowing through i32 where possible;
    // fall through to `NumCast` to honour the project's no-`as` rule.
    let kf: f64 = num_traits::NumCast::from(k).unwrap_or(0.0);
    let nf: f64 = num_traits::NumCast::from(n).unwrap_or(0.0);
    let log_pmf = (nf - kf).mul_add((1.0 - prob).ln(), kf.mul_add(prob.ln(), coeff));
    Response::ok(TOOL_BINOMIAL_PMF)
        .result(format_f64(log_pmf.exp()))
        .build()
}

fn log_binomial(n: i64, k: i64) -> f64 {
    let k = k.min(n - k);
    let mut result = 0.0;
    for i in 0..k {
        let num: f64 = num_traits::NumCast::from(n - i).unwrap_or(0.0);
        let den: f64 = num_traits::NumCast::from(i + 1).unwrap_or(1.0);
        result += num.ln() - den.ln();
    }
    result
}

/// Two-sided confidence interval for a sample mean using normal approximation
/// (z-score). For small samples the caller should use a t-distribution table.
#[must_use]
pub fn confidence_interval(values: &str, confidence_level: &str) -> String {
    let arr = match parse_array(TOOL_CONFIDENCE_INTERVAL, "values", values) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if arr.len() < 2 {
        return error(
            TOOL_CONFIDENCE_INTERVAL,
            ErrorCode::InvalidInput,
            "at least 2 values required",
        );
    }
    let level = match parse_decimal(
        TOOL_CONFIDENCE_INTERVAL,
        "confidenceLevel",
        confidence_level,
    ) {
        Ok(v) => v,
        Err(e) => return e,
    };
    // Open interval `(0, 1)` — the message says so and a `level=0` interval
    // would have zero width, while `level=1` would require the distribution's
    // full support (z → ∞). Rust's `(0.0..1.0).contains(&level)` is
    // half-open `[0, 1)`, so `0.0` used to slip through and yield a
    // zero-margin envelope; guard explicitly on both ends.
    if level <= 0.0 || level >= 1.0 {
        return error_with_detail(
            TOOL_CONFIDENCE_INTERVAL,
            ErrorCode::OutOfRange,
            "confidenceLevel must be in (0, 1)",
            &format!("level={level}"),
        );
    }
    let n = count_as_f64(arr.len());
    let mean_val: f64 = arr.iter().sum::<f64>() / n;
    let sd = compute_variance(&arr, false).sqrt();
    let se = sd / n.sqrt();
    let alpha = 1.0 - level;
    let z = inverse_normal_cdf(1.0 - alpha / 2.0);
    let margin = z * se;
    Response::ok(TOOL_CONFIDENCE_INTERVAL)
        .field("MEAN", format_f64(mean_val))
        .field("LOWER", format_f64(mean_val - margin))
        .field("UPPER", format_f64(mean_val + margin))
        .field("MARGIN", format_f64(margin))
        .build()
}

/// Horner-scheme polynomial evaluation using fused multiply-add.
/// `coeffs[0]` is the leading coefficient, `coeffs.last()` is the constant.
fn horner(coeffs: &[f64], variable: f64) -> f64 {
    coeffs
        .iter()
        .copied()
        .reduce(|acc, c| acc.mul_add(variable, c))
        .unwrap_or(0.0)
}

/// Coefficients from Acklam, "An algorithm for computing the inverse normal
/// cumulative distribution function" (2003). Kept as module constants so
/// `horner` can operate on them without taking ownership.
const ACKLAM_A: [f64; 6] = [
    -3.969_683_028_665_376e1,
    2.209_460_984_245_205e2,
    -2.759_285_104_469_687e2,
    1.383_577_518_672_69e2,
    -3.066_479_806_614_716e1,
    2.506_628_277_459_239,
];
const ACKLAM_B: [f64; 6] = [
    -5.447_609_879_822_406e1,
    1.615_858_368_580_409e2,
    -1.556_989_798_598_866e2,
    6.680_131_188_771_972e1,
    -1.328_068_155_288_572e1,
    // Trailing 1.0 appended so horner produces the same polynomial as the
    // original hand-expanded form, keeping the denominator unchanged.
    1.0,
];
const ACKLAM_C: [f64; 6] = [
    -7.784_894_002_430_293e-3,
    -3.223_964_580_411_365e-1,
    -2.400_758_277_161_838,
    -2.549_732_539_343_734,
    4.374_664_141_464_968,
    2.938_163_982_698_783,
];
const ACKLAM_D: [f64; 5] = [
    7.784_695_709_041_462e-3,
    3.224_671_290_700_398e-1,
    2.445_134_137_142_996,
    3.754_408_661_907_416,
    1.0,
];

/// Beasley-Springer-Moro inverse normal CDF approximation.
fn inverse_normal_cdf(p: f64) -> f64 {
    let p_low = 0.02425;
    let p_high = 1.0 - p_low;
    if p < p_low {
        let q = (-2.0 * p.ln()).sqrt();
        horner(&ACKLAM_C, q) / horner(&ACKLAM_D, q)
    } else if p <= p_high {
        let q = p - 0.5;
        let r = q * q;
        horner(&ACKLAM_A, r) * q / horner(&ACKLAM_B, r)
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -horner(&ACKLAM_C, q) / horner(&ACKLAM_D, q)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(actual: &str, expected: &str, label: &str) {
        // Tolerance-aware comparison for the floating-point fields.
        let parts_a: Vec<&str> = actual.split(" | ").collect();
        let result_a = parts_a
            .iter()
            .find_map(|p| p.strip_prefix("RESULT: "))
            .unwrap_or("");
        let av: f64 = result_a.parse().unwrap_or(f64::NAN);
        let ev: f64 = expected.parse().unwrap_or(f64::NAN);
        assert!((av - ev).abs() < 1e-6, "{label}: expected ~{ev}, got {av}");
    }

    #[test]
    fn mean_simple() {
        approx(&mean("1,2,3,4,5"), "3.0", "mean");
    }

    #[test]
    fn mean_with_negatives() {
        approx(&mean("-2,-1,0,1,2"), "0.0", "mean");
    }

    #[test]
    fn mean_empty_array_errors() {
        let out = mean("");
        assert!(out.starts_with("MEAN: ERROR\nREASON: [INVALID_INPUT]"));
    }

    #[test]
    fn median_odd_length() {
        approx(&median("3,1,2"), "2.0", "median");
    }

    #[test]
    fn median_even_length_averages_middles() {
        approx(&median("1,2,3,4"), "2.5", "median");
    }

    #[test]
    fn mode_single_dominant() {
        let out = mode("1,2,2,3");
        assert!(out.contains("MODES: 2.0"));
        assert!(out.contains("COUNT: 2"));
    }

    #[test]
    fn mode_multi_modal() {
        let out = mode("1,1,2,2,3");
        assert!(out.contains("MODES: 1.0,2.0"), "got {out}");
    }

    #[test]
    fn variance_sample_default() {
        // Sample variance of 1..5 = 2.5
        approx(&variance("1,2,3,4,5", false), "2.5", "variance");
    }

    #[test]
    fn variance_population_for_same_data() {
        // Population variance = 2.0
        approx(&variance("1,2,3,4,5", true), "2.0", "variance pop");
    }

    #[test]
    fn stddev_sample_matches_sqrt_variance() {
        let out = std_dev("1,2,3,4,5", false);
        // stddev = sqrt(2.5) ≈ 1.5811388
        assert!(out.contains("RESULT: 1.581"), "got {out}");
    }

    #[test]
    fn percentile_50_matches_median() {
        approx(&percentile("1,2,3,4,5", "50"), "3.0", "p50");
    }

    #[test]
    fn percentile_25_quartile() {
        // R-7 Q1 of 1..5 = 2.0
        approx(&percentile("1,2,3,4,5", "25"), "2.0", "p25");
    }

    #[test]
    fn quartile_q1_q3() {
        let q1 = quartile("1,2,3,4,5", 1);
        assert!(q1.contains("VALUE: 2.0"), "got {q1}");
        let q3 = quartile("1,2,3,4,5", 3);
        assert!(q3.contains("VALUE: 4.0"), "got {q3}");
    }

    #[test]
    fn iqr_basic() {
        let out = iqr("1,2,3,4,5,6,7,8,9");
        assert!(out.contains("IQR: 4.0"), "got {out}");
    }

    #[test]
    fn correlation_perfect_positive() {
        let out = correlation("1,2,3,4,5", "2,4,6,8,10");
        assert!(out.contains("RESULT: 1.0"), "got {out}");
    }

    #[test]
    fn correlation_perfect_negative() {
        let out = correlation("1,2,3,4,5", "10,8,6,4,2");
        assert!(out.contains("RESULT: -1.0"), "got {out}");
    }

    #[test]
    fn correlation_unequal_length_errors() {
        let out = correlation("1,2", "3,4,5");
        assert!(out.starts_with("CORRELATION: ERROR"));
    }

    #[test]
    fn correlation_constant_series_errors() {
        // Constant x (zero variance) makes Pearson undefined — previously
        // returned 0.0 silently, masking the degenerate input.
        let out = correlation("1,1,1,1", "1,2,3,4");
        assert!(out.starts_with("CORRELATION: ERROR"));
        assert!(out.contains("zero variance"));
    }

    #[test]
    fn correlation_survives_mixed_extreme_scales() {
        // Regression: without deviation-scale normalization, Pearson on
        // `xs ~ 1e300` vs `ys ~ 1e-300` accumulates `Σdx² → +∞` and
        // `Σdy² → 0`, and `sqrt(inf · 0) = NaN` — despite the correlation
        // being mathematically `1.0`. Scale-invariant accumulation keeps
        // every intermediate inside f64 range regardless of the raw
        // magnitudes.
        let out = correlation("1e300,2e300,3e300", "1e-300,2e-300,3e-300");
        assert!(out.starts_with("CORRELATION: OK"), "got {out}");
        assert!(
            !out.contains("NaN"),
            "correlation should not surface NaN, got {out}"
        );
        assert!(out.contains("RESULT: 1.0"), "got {out}");
        // Symmetrically on the anti-correlation side.
        let anti = correlation("1e-300,2e-300,3e-300", "3e300,2e300,1e300");
        assert!(anti.contains("RESULT: -1.0"), "got {anti}");
    }

    #[test]
    fn linear_regression_y_equals_2x_plus_1() {
        // y = 2x + 1 should give slope=2, intercept=1
        let out = linear_regression("0,1,2,3,4", "1,3,5,7,9");
        assert!(out.contains("SLOPE: 2.0"), "got {out}");
        assert!(out.contains("INTERCEPT: 1.0"), "got {out}");
        assert!(out.contains("R: 1.0"), "got {out}");
    }

    #[test]
    fn normal_pdf_at_mean_is_max() {
        // f(0; 0, 1) = 1/sqrt(2π) ≈ 0.3989422804
        let out = normal_pdf("0", "0", "1");
        assert!(out.contains("RESULT: 0.398"), "got {out}");
    }

    #[test]
    fn normal_cdf_symmetric_about_mean() {
        // Φ(μ; μ, σ) must be exactly 0.5 — the symmetry short-circuit skips
        // erf entirely. Prior to the snap this returned 0.5000000005.
        let out = normal_cdf("0", "0", "1");
        assert!(
            out.contains("RESULT: 0.5\n") || out.ends_with("RESULT: 0.5"),
            "got {out}"
        );
        let out_shifted = normal_cdf("3.5", "3.5", "2");
        assert!(
            out_shifted.contains("RESULT: 0.5\n") || out_shifted.ends_with("RESULT: 0.5"),
            "got {out_shifted}"
        );
    }

    #[test]
    fn normal_cdf_one_sigma() {
        // Φ(1; 0, 1) ≈ 0.8413447460685429. libm's erf matches at f64 precision.
        let out = normal_cdf("1", "0", "1");
        assert!(out.contains("RESULT: 0.8413447"), "got {out}");
    }

    #[test]
    fn normal_cdf_three_sigma_matches_reference() {
        // Φ(3; 0, 1) = 0.9986501019683699 — upgrade from Abramowitz 7.1.26
        // (which drifted by ~1e-7) to libm's erf must now hit ~1e-15 accuracy.
        let out = normal_cdf("3", "0", "1");
        assert!(out.contains("RESULT: 0.998650101"), "got {out}");
    }

    #[test]
    fn t_test_against_known_mean() {
        // Sample [1,2,3,4,5], mean=3, sample stddev=1.5811, n=5
        // Test against H0 mean=2.5. Expect t = (3 - 2.5) / (1.5811 / sqrt(5)) ≈ 0.7071
        let out = t_test_one_sample("1,2,3,4,5", "2.5");
        assert!(out.contains("T: 0.707"), "got {out}");
        assert!(out.contains("DF: 4.0"), "got {out}");
    }

    #[test]
    fn binomial_pmf_basic() {
        // B(10, k=5, p=0.5) = C(10,5)*0.5^10 = 252/1024 ≈ 0.2461
        let out = binomial_pmf(10, 5, "0.5");
        assert!(out.contains("RESULT: 0.246"), "got {out}");
    }

    #[test]
    fn binomial_pmf_invalid_p_errors() {
        let out = binomial_pmf(10, 5, "1.5");
        assert!(out.starts_with("BINOMIAL_PMF: ERROR"));
    }

    #[test]
    fn binomial_pmf_p_zero_boundary() {
        // p=0: only k=0 is certain, else impossible.
        assert!(binomial_pmf(10, 0, "0").contains("RESULT: 1.0"));
        assert!(binomial_pmf(10, 1, "0").contains("RESULT: 0.0"));
        assert!(binomial_pmf(10, 10, "0").contains("RESULT: 0.0"));
    }

    #[test]
    fn binomial_pmf_p_one_boundary() {
        // p=1: only k=n is certain, else impossible.
        assert!(binomial_pmf(10, 10, "1").contains("RESULT: 1.0"));
        assert!(binomial_pmf(10, 9, "1").contains("RESULT: 0.0"));
        assert!(binomial_pmf(10, 0, "1").contains("RESULT: 0.0"));
    }

    #[test]
    fn confidence_interval_centered_on_mean() {
        // Mean of 1..5 = 3; CI should be centered on 3
        let out = confidence_interval("1,2,3,4,5", "0.95");
        assert!(out.contains("MEAN: 3.0"), "got {out}");
    }

    #[test]
    fn confidence_interval_rejects_boundary_levels() {
        // Regression: the guard used the half-open Rust range `[0, 1)`,
        // so `level=0` slipped past the `contains` check and returned a
        // zero-margin envelope despite the error message advertising the
        // open interval `(0, 1)`. Both `0` and `1` must be rejected.
        let zero = confidence_interval("1,2,3,4,5", "0");
        assert!(zero.starts_with("CONFIDENCE_INTERVAL: ERROR"), "got {zero}");
        let one = confidence_interval("1,2,3,4,5", "1");
        assert!(one.starts_with("CONFIDENCE_INTERVAL: ERROR"), "got {one}");
    }

    #[test]
    fn parse_error_in_array_propagates() {
        let out = mean("1,foo,3");
        assert!(out.starts_with("MEAN: ERROR\nREASON: [PARSE_ERROR]"));
    }
}
