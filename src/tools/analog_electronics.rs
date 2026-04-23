//! Analog electronics tooling — Ohm's law, reactive combinations, dividers,
//! time constants, resonance, impedance, dB, filter cutoff, LED resistor,
//! Wheatstone bridge.
//!
//! All public functions return `String` and produce a
//! `crate::mcp::message` envelope (inline for success, block-style error for
//! failures).

use std::cell::RefCell;
use std::num::NonZeroU64;
use std::str::FromStr;

use astro_float::{BigFloat, Consts, Radix, RoundingMode as AfRm};
use bigdecimal::{BigDecimal, RoundingMode};
use num_traits::{Signed, Zero};

use crate::engine::bigdecimal_ext::{DECIMAL128_PRECISION, DIVISION_SCALE, TWO_PI, strip_plain};
use crate::mcp::message::{ErrorCode, Response, error, error_with_detail};

// ------------------------------------------------------------------ //
//  Tool names
// ------------------------------------------------------------------ //

const OHMS_LAW: &str = "OHMS_LAW";
const RESISTOR_COMBINATION: &str = "RESISTOR_COMBINATION";
const CAPACITOR_COMBINATION: &str = "CAPACITOR_COMBINATION";
const INDUCTOR_COMBINATION: &str = "INDUCTOR_COMBINATION";
const VOLTAGE_DIVIDER: &str = "VOLTAGE_DIVIDER";
const CURRENT_DIVIDER: &str = "CURRENT_DIVIDER";
const RC_TIME_CONSTANT: &str = "RC_TIME_CONSTANT";
const RL_TIME_CONSTANT: &str = "RL_TIME_CONSTANT";
const RLC_RESONANCE: &str = "RLC_RESONANCE";
const IMPEDANCE: &str = "IMPEDANCE";
const DECIBEL_CONVERT: &str = "DECIBEL_CONVERT";
const FILTER_CUTOFF: &str = "FILTER_CUTOFF";
const LED_RESISTOR: &str = "LED_RESISTOR";
const WHEATSTONE_BRIDGE: &str = "WHEATSTONE_BRIDGE";

const SERIES: &str = "series";
const PARALLEL: &str = "parallel";
const REQUIRED_KNOWNS: u32 = 2;
const AF_PRECISION: usize = 128;

thread_local! {
    static AF_CONSTS: RefCell<Consts> =
        RefCell::new(Consts::new().expect("failed to initialize astro-float Consts"));
}

const fn precision() -> NonZeroU64 {
    NonZeroU64::new(DECIMAL128_PRECISION).expect("precision is non-zero")
}

fn mul_ctx(lhs: &BigDecimal, rhs: &BigDecimal) -> BigDecimal {
    (lhs * rhs).with_precision_round(precision(), RoundingMode::HalfUp)
}

fn add_ctx(lhs: &BigDecimal, rhs: &BigDecimal) -> BigDecimal {
    (lhs + rhs).with_precision_round(precision(), RoundingMode::HalfUp)
}

fn sub_ctx(lhs: &BigDecimal, rhs: &BigDecimal) -> BigDecimal {
    (lhs - rhs).with_precision_round(precision(), RoundingMode::HalfUp)
}

fn div_scaled(lhs: &BigDecimal, rhs: &BigDecimal) -> BigDecimal {
    (lhs / rhs).with_scale_round(DIVISION_SCALE, RoundingMode::HalfUp)
}

// --- astro-float bridge helpers ---

fn bd_to_bf(value: &BigDecimal) -> BigFloat {
    AF_CONSTS.with(|cc| {
        BigFloat::parse(
            &value.to_plain_string(),
            Radix::Dec,
            AF_PRECISION,
            AfRm::None,
            &mut cc.borrow_mut(),
        )
    })
}

fn bf_to_bd(value: &BigFloat) -> BigDecimal {
    let formatted = AF_CONSTS.with(|cc| {
        value
            .format(Radix::Dec, AfRm::ToEven, &mut cc.borrow_mut())
            .expect("astro-float format failed")
    });
    BigDecimal::from_str(&formatted).expect("astro-float output parses as BigDecimal")
}

fn af_sqrt(value: &BigDecimal) -> BigDecimal {
    let bf = bd_to_bf(value);
    bf_to_bd(&bf.sqrt(AF_PRECISION, AfRm::ToEven))
}

fn af_log10(value: &BigDecimal) -> BigDecimal {
    let bf = bd_to_bf(value);
    let out = AF_CONSTS.with(|cc| bf.log10(AF_PRECISION, AfRm::ToEven, &mut cc.borrow_mut()));
    bf_to_bd(&out)
}

fn af_pow10(exponent: &BigDecimal) -> BigDecimal {
    let base = AF_CONSTS.with(|cc| {
        BigFloat::parse(
            "10",
            Radix::Dec,
            AF_PRECISION,
            AfRm::None,
            &mut cc.borrow_mut(),
        )
    });
    let exp_bf = bd_to_bf(exponent);
    let out =
        AF_CONSTS.with(|cc| base.pow(&exp_bf, AF_PRECISION, AfRm::ToEven, &mut cc.borrow_mut()));
    bf_to_bd(&out)
}

/// Ratio `|y| / |x|` beyond which the `y/x` quotient used inside
/// `af_atan2_degrees` loses precision catastrophically: astro-float's atan
/// implementation drifts toward `3π/2` instead of the expected `±π/2` once
/// the quotient exceeds its internal exponent window. Well before then
/// (`|y|/|x| ≥ 10^40`) the atan has converged to machine-precision `±π/2`
/// anyway, so we short-circuit and skip the quotient entirely. This bound
/// is orders of magnitude above any physically realistic impedance
/// measurement yet stays clear of the library's bad zone.
const ATAN2_DEGENERATE_RATIO_DIGITS: u32 = 40;

/// True when `|y|` overwhelms `|x|` by more than `10^ATAN2_DEGENERATE_RATIO_DIGITS`.
/// In that regime `atan(y/x) ≈ ±π/2` to full precision, and we can bypass
/// the astro-float `y/x` + `atan` chain that would otherwise mis-round.
fn atan2_is_degenerate(y: &BigDecimal, x: &BigDecimal) -> bool {
    if x.is_zero() {
        return true;
    }
    let threshold = BigDecimal::from(10u64)
        .with_scale(0)
        .pow(ATAN2_DEGENERATE_RATIO_DIGITS);
    y.abs() > x.abs() * threshold
}

trait BigDecPow {
    fn pow(&self, exp: u32) -> BigDecimal;
}

impl BigDecPow for BigDecimal {
    fn pow(&self, exp: u32) -> Self {
        let mut result = Self::from(1);
        for _ in 0..exp {
            result *= self;
        }
        result
    }
}

/// atan2 via atan + quadrant adjust, result in degrees.
fn af_atan2_degrees(y: &BigDecimal, x: &BigDecimal) -> BigDecimal {
    const PI_LITERAL: &str =
        "3.14159265358979323846264338327950288419716939937510582097494459230781640628620";
    // Pre-check: treat x as effectively zero when |y| ≫ |x|. Without this
    // guard, astro-float's `y/x` becomes a magnitude it cannot represent and
    // the subsequent `atan` mis-rounds (regression: `impedance(1e-200,
    // 1e-200, 1e-200, 1e200)` reported phase 270° instead of 90°).
    if atan2_is_degenerate(y, x) {
        let sign_y: i32 = if y.is_negative() { -1 } else { 1 };
        let ninety =
            BigDecimal::from_str("90").expect("valid 90 literal") * BigDecimal::from(sign_y);
        return ninety;
    }
    let y_bf = bd_to_bf(y);
    let x_bf = bd_to_bf(x);
    let radians = AF_CONSTS.with(|cc| {
        let mut consts = cc.borrow_mut();
        let pi = BigFloat::parse(
            PI_LITERAL,
            Radix::Dec,
            AF_PRECISION,
            AfRm::None,
            &mut consts,
        );
        let two = BigFloat::parse("2", Radix::Dec, AF_PRECISION, AfRm::None, &mut consts);
        let zero = BigFloat::parse("0", Radix::Dec, AF_PRECISION, AfRm::None, &mut consts);
        let half_pi = pi.div(&two, AF_PRECISION, AfRm::ToEven);
        let x_cmp = x_bf.cmp(&zero).unwrap_or(0);
        let y_cmp = y_bf.cmp(&zero).unwrap_or(0);
        if x_cmp > 0 {
            y_bf.div(&x_bf, AF_PRECISION, AfRm::ToEven).atan(
                AF_PRECISION,
                AfRm::ToEven,
                &mut consts,
            )
        } else if x_cmp < 0 && y_cmp >= 0 {
            let base = y_bf.div(&x_bf, AF_PRECISION, AfRm::ToEven).atan(
                AF_PRECISION,
                AfRm::ToEven,
                &mut consts,
            );
            base.add(&pi, AF_PRECISION, AfRm::ToEven)
        } else if x_cmp < 0 {
            let base = y_bf.div(&x_bf, AF_PRECISION, AfRm::ToEven).atan(
                AF_PRECISION,
                AfRm::ToEven,
                &mut consts,
            );
            base.sub(&pi, AF_PRECISION, AfRm::ToEven)
        } else if y_cmp >= 0 {
            half_pi
        } else {
            half_pi.neg()
        }
    });
    let rad_bd = bf_to_bd(&radians);
    let deg_per_rad = BigDecimal::from_str("57.29577951308232087679815481410517033240")
        .expect("valid 180/pi literal");
    mul_ctx(&rad_bd, &deg_per_rad)
}

// --- Parsing helpers ---

fn parse_bd(tool: &str, raw: &str, label: &str) -> Result<BigDecimal, String> {
    BigDecimal::from_str(raw.trim()).map_err(|_| {
        error_with_detail(
            tool,
            ErrorCode::ParseError,
            &format!("{label} is not a valid decimal number"),
            &format!("{label}={raw}"),
        )
    })
}

const fn is_present(value: &str) -> bool {
    !value.is_empty()
}

fn non_zero(tool: &str, value: &BigDecimal, name: &str) -> Result<(), String> {
    if value.is_zero() {
        Err(error(
            tool,
            ErrorCode::DivisionByZero,
            &format!("{name} must not be zero"),
        ))
    } else {
        Ok(())
    }
}

/// Reject non-positive values. `name` is used both as the lowercase subject
/// in the reason text ("<name> must be positive") and as the DETAIL key. It
/// must not contain whitespace — pick a single-token identifier that matches
/// the MCP parameter name (e.g. `resistance`, `capacitance`).
fn positive(tool: &str, value: &BigDecimal, name: &str) -> Result<(), String> {
    debug_assert!(
        !name.contains(char::is_whitespace),
        "positive() name must be a single token",
    );
    if value.is_zero() || value.is_negative() {
        Err(error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            &format!("{name} must be positive"),
            &format!("{name}={}", strip_plain(value)),
        ))
    } else {
        Ok(())
    }
}

fn parse_csv(tool: &str, values: &str) -> Result<Vec<BigDecimal>, String> {
    let trimmed = values.trim();
    if trimmed.is_empty() {
        return Err(error(
            tool,
            ErrorCode::InvalidInput,
            "at least one value is required",
        ));
    }
    let parts: Vec<&str> = trimmed.split(',').collect();
    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        let piece = part.trim();
        if piece.is_empty() {
            return Err(error_with_detail(
                tool,
                ErrorCode::InvalidInput,
                "empty value in CSV list",
                &format!("values={values}"),
            ));
        }
        out.push(parse_bd(tool, piece, "value")?);
    }
    Ok(out)
}

fn validate_filter_type(tool: &str, filter_type: &str) -> Result<String, String> {
    let lower = filter_type.to_ascii_lowercase();
    if lower == "lowpass" || lower == "highpass" {
        Ok(lower)
    } else {
        Err(error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            "filter type must be 'lowpass' or 'highpass'",
            &format!("filter={filter_type}"),
        ))
    }
}

// --- Ohm's law ---

/// Ohm's Law: provide exactly two of V/I/R/P (non-empty) and compute the rest.
#[must_use]
pub fn ohms_law(voltage: &str, current: &str, resistance: &str, power: &str) -> String {
    let has_v = is_present(voltage);
    let has_i = is_present(current);
    let has_r = is_present(resistance);
    let has_p = is_present(power);
    let count = u32::from(has_v) + u32::from(has_i) + u32::from(has_r) + u32::from(has_p);
    if count != REQUIRED_KNOWNS {
        return error(
            OHMS_LAW,
            ErrorCode::InvalidInput,
            "exactly two of V, I, R, P must be provided",
        );
    }
    match dispatch_ohms(has_v, has_i, has_r, voltage, current, resistance, power) {
        Ok(values) => ohms_envelope(&values),
        Err(e) => e,
    }
}

struct OhmsValues {
    voltage: BigDecimal,
    current: BigDecimal,
    resistance: BigDecimal,
    power: BigDecimal,
}

fn dispatch_ohms(
    has_v: bool,
    has_i: bool,
    has_r: bool,
    volt: &str,
    curr: &str,
    res: &str,
    pow: &str,
) -> Result<OhmsValues, String> {
    let key = (u8::from(has_v) << 2) | (u8::from(has_i) << 1) | u8::from(has_r);
    match key {
        6 => ohms_from_vi(
            parse_bd(OHMS_LAW, volt, "voltage")?,
            parse_bd(OHMS_LAW, curr, "current")?,
        ),
        5 => ohms_from_vr(
            parse_bd(OHMS_LAW, volt, "voltage")?,
            parse_bd(OHMS_LAW, res, "resistance")?,
        ),
        4 => ohms_from_vp(
            parse_bd(OHMS_LAW, volt, "voltage")?,
            parse_bd(OHMS_LAW, pow, "power")?,
        ),
        3 => ohms_from_ir(
            parse_bd(OHMS_LAW, curr, "current")?,
            parse_bd(OHMS_LAW, res, "resistance")?,
        ),
        2 => ohms_from_ip(
            parse_bd(OHMS_LAW, curr, "current")?,
            parse_bd(OHMS_LAW, pow, "power")?,
        ),
        _ => ohms_from_rp(
            parse_bd(OHMS_LAW, res, "resistance")?,
            parse_bd(OHMS_LAW, pow, "power")?,
        ),
    }
}

fn non_negative(tool: &str, value: &BigDecimal, name: &str) -> Result<(), String> {
    if value.is_negative() {
        Err(error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            &format!("{name} must not be negative"),
            &format!("{name}={}", strip_plain(value)),
        ))
    } else {
        Ok(())
    }
}

fn ohms_from_vi(voltage: BigDecimal, current: BigDecimal) -> Result<OhmsValues, String> {
    non_negative(OHMS_LAW, &voltage, "voltage")?;
    non_negative(OHMS_LAW, &current, "current")?;
    non_zero(OHMS_LAW, &current, "current")?;
    let resistance = div_scaled(&voltage, &current);
    let power = mul_ctx(&voltage, &current);
    Ok(OhmsValues {
        voltage,
        current,
        resistance,
        power,
    })
}

fn ohms_from_vr(voltage: BigDecimal, resistance: BigDecimal) -> Result<OhmsValues, String> {
    non_negative(OHMS_LAW, &voltage, "voltage")?;
    non_negative(OHMS_LAW, &resistance, "resistance")?;
    non_zero(OHMS_LAW, &resistance, "resistance")?;
    let current = div_scaled(&voltage, &resistance);
    let power = mul_ctx(&voltage, &current);
    Ok(OhmsValues {
        voltage,
        current,
        resistance,
        power,
    })
}

fn ohms_from_vp(voltage: BigDecimal, power: BigDecimal) -> Result<OhmsValues, String> {
    non_negative(OHMS_LAW, &voltage, "voltage")?;
    non_negative(OHMS_LAW, &power, "power")?;
    non_zero(OHMS_LAW, &voltage, "voltage")?;
    let current = div_scaled(&power, &voltage);
    let resistance = div_scaled(&voltage, &current);
    Ok(OhmsValues {
        voltage,
        current,
        resistance,
        power,
    })
}

fn ohms_from_ir(current: BigDecimal, resistance: BigDecimal) -> Result<OhmsValues, String> {
    non_negative(OHMS_LAW, &current, "current")?;
    non_negative(OHMS_LAW, &resistance, "resistance")?;
    let voltage = mul_ctx(&current, &resistance);
    let power = mul_ctx(&voltage, &current);
    Ok(OhmsValues {
        voltage,
        current,
        resistance,
        power,
    })
}

fn ohms_from_ip(current: BigDecimal, power: BigDecimal) -> Result<OhmsValues, String> {
    non_negative(OHMS_LAW, &current, "current")?;
    non_negative(OHMS_LAW, &power, "power")?;
    non_zero(OHMS_LAW, &current, "current")?;
    let voltage = div_scaled(&power, &current);
    let resistance = div_scaled(&voltage, &current);
    Ok(OhmsValues {
        voltage,
        current,
        resistance,
        power,
    })
}

fn ohms_from_rp(resistance: BigDecimal, power: BigDecimal) -> Result<OhmsValues, String> {
    non_negative(OHMS_LAW, &resistance, "resistance")?;
    non_negative(OHMS_LAW, &power, "power")?;
    non_zero(OHMS_LAW, &resistance, "resistance")?;
    let pr_product = mul_ctx(&power, &resistance);
    let voltage = af_sqrt(&pr_product);
    let pr_ratio = div_scaled(&power, &resistance);
    let current = af_sqrt(&pr_ratio);
    Ok(OhmsValues {
        voltage,
        current,
        resistance,
        power,
    })
}

fn ohms_envelope(values: &OhmsValues) -> String {
    Response::ok(OHMS_LAW)
        .field("VOLTAGE", strip_plain(&values.voltage))
        .field("CURRENT", strip_plain(&values.current))
        .field("RESISTANCE", strip_plain(&values.resistance))
        .field("POWER", strip_plain(&values.power))
        .build()
}

// --- Resistor / Capacitor / Inductor combination ---

/// Resistor combination: series sums, parallel reciprocal-sums.
#[must_use]
pub fn resistor_combination(values: &str, mode: &str) -> String {
    match combine(RESISTOR_COMBINATION, values, mode, false) {
        Ok(val) => Response::ok(RESISTOR_COMBINATION)
            .result(strip_plain(&val))
            .build(),
        Err(e) => e,
    }
}

/// Capacitor combination: series reciprocal-sums, parallel sums (reversed from R/L).
#[must_use]
pub fn capacitor_combination(values: &str, mode: &str) -> String {
    match combine(CAPACITOR_COMBINATION, values, mode, true) {
        Ok(val) => Response::ok(CAPACITOR_COMBINATION)
            .result(strip_plain(&val))
            .build(),
        Err(e) => e,
    }
}

/// Inductor combination: same as resistor (series sums, parallel reciprocal-sums).
#[must_use]
pub fn inductor_combination(values: &str, mode: &str) -> String {
    match combine(INDUCTOR_COMBINATION, values, mode, false) {
        Ok(val) => Response::ok(INDUCTOR_COMBINATION)
            .result(strip_plain(&val))
            .build(),
        Err(e) => e,
    }
}

fn combine(tool: &str, values: &str, mode: &str, reversed: bool) -> Result<BigDecimal, String> {
    let parsed = parse_csv(tool, values)?;
    let lower = mode.to_ascii_lowercase();
    let use_sum = match (lower.as_str(), reversed) {
        (SERIES, false) | (PARALLEL, true) => true,
        (PARALLEL, false) | (SERIES, true) => false,
        _ => {
            return Err(error_with_detail(
                tool,
                ErrorCode::InvalidInput,
                "mode must be 'series' or 'parallel'",
                &format!("mode={mode}"),
            ));
        }
    };
    if use_sum {
        sum_values(tool, &parsed)
    } else {
        reciprocal_sum(tool, &parsed)
    }
}

fn sum_values(tool: &str, values: &[BigDecimal]) -> Result<BigDecimal, String> {
    let mut total = BigDecimal::zero();
    for val in values {
        if val.is_negative() {
            return Err(error(
                tool,
                ErrorCode::InvalidInput,
                "component value must not be negative",
            ));
        }
        total = add_ctx(&total, val);
    }
    Ok(total)
}

fn reciprocal_sum(tool: &str, values: &[BigDecimal]) -> Result<BigDecimal, String> {
    let one = BigDecimal::from(1);
    let mut reciprocal = BigDecimal::zero();
    for val in values {
        if val.is_zero() || val.is_negative() {
            return Err(error(
                tool,
                ErrorCode::InvalidInput,
                "component value must be positive",
            ));
        }
        let r = div_scaled(&one, val);
        reciprocal = add_ctx(&reciprocal, &r);
    }
    non_zero(tool, &reciprocal, "reciprocal sum")?;
    Ok(div_scaled(&one, &reciprocal))
}

// --- Dividers ---

/// Voltage divider: Vout = Vin * R2 / (R1 + R2).
#[must_use]
pub fn voltage_divider(vin: &str, r1: &str, r2: &str) -> String {
    let vin_v = match parse_bd(VOLTAGE_DIVIDER, vin, "vin") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let r1_v = match parse_bd(VOLTAGE_DIVIDER, r1, "r1") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let r2_v = match parse_bd(VOLTAGE_DIVIDER, r2, "r2") {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = non_negative(VOLTAGE_DIVIDER, &vin_v, "vin") {
        return e;
    }
    if let Err(e) = positive(VOLTAGE_DIVIDER, &r1_v, "r1") {
        return e;
    }
    if let Err(e) = positive(VOLTAGE_DIVIDER, &r2_v, "r2") {
        return e;
    }
    let sum = add_ctx(&r1_v, &r2_v);
    let vout = div_scaled(&mul_ctx(&vin_v, &r2_v), &sum);
    Response::ok(VOLTAGE_DIVIDER)
        .field("VOUT", strip_plain(&vout))
        .build()
}

/// Current divider: I1 = It*R2/(R1+R2), I2 = It*R1/(R1+R2).
#[must_use]
pub fn current_divider(total_current: &str, r1: &str, r2: &str) -> String {
    let it = match parse_bd(CURRENT_DIVIDER, total_current, "totalCurrent") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let r1_v = match parse_bd(CURRENT_DIVIDER, r1, "r1") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let r2_v = match parse_bd(CURRENT_DIVIDER, r2, "r2") {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = non_negative(CURRENT_DIVIDER, &it, "totalCurrent") {
        return e;
    }
    if let Err(e) = positive(CURRENT_DIVIDER, &r1_v, "r1") {
        return e;
    }
    if let Err(e) = positive(CURRENT_DIVIDER, &r2_v, "r2") {
        return e;
    }
    let sum = add_ctx(&r1_v, &r2_v);
    let i1 = div_scaled(&mul_ctx(&it, &r2_v), &sum);
    let i2 = div_scaled(&mul_ctx(&it, &r1_v), &sum);
    Response::ok(CURRENT_DIVIDER)
        .field("I1", strip_plain(&i1))
        .field("I2", strip_plain(&i2))
        .build()
}

// --- Time constants ---

/// RC time constant and cutoff frequency.
#[must_use]
pub fn rc_time_constant(resistance: &str, capacitance: &str) -> String {
    let r = match parse_bd(RC_TIME_CONSTANT, resistance, "resistance") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let c = match parse_bd(RC_TIME_CONSTANT, capacitance, "capacitance") {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = positive(RC_TIME_CONSTANT, &r, "resistance") {
        return e;
    }
    if let Err(e) = positive(RC_TIME_CONSTANT, &c, "capacitance") {
        return e;
    }
    let tau = mul_ctx(&r, &c);
    let denom = mul_ctx(&TWO_PI, &tau);
    let freq = div_scaled(&BigDecimal::from(1), &denom);
    Response::ok(RC_TIME_CONSTANT)
        .field("TAU", strip_plain(&tau))
        .field("CUTOFF_FREQUENCY", strip_plain(&freq))
        .build()
}

/// RL time constant and cutoff frequency.
#[must_use]
pub fn rl_time_constant(resistance: &str, inductance: &str) -> String {
    let r = match parse_bd(RL_TIME_CONSTANT, resistance, "resistance") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let l = match parse_bd(RL_TIME_CONSTANT, inductance, "inductance") {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = positive(RL_TIME_CONSTANT, &r, "resistance") {
        return e;
    }
    if let Err(e) = positive(RL_TIME_CONSTANT, &l, "inductance") {
        return e;
    }
    let tau = div_scaled(&l, &r);
    let denom = mul_ctx(&TWO_PI, &l);
    let freq = div_scaled(&r, &denom);
    Response::ok(RL_TIME_CONSTANT)
        .field("TAU", strip_plain(&tau))
        .field("CUTOFF_FREQUENCY", strip_plain(&freq))
        .build()
}

// --- RLC / impedance ---

struct RlcInputs {
    r: BigDecimal,
    l: BigDecimal,
    c: BigDecimal,
}

fn parse_rlc_positive(r: &str, l: &str, c: &str) -> Result<RlcInputs, String> {
    let r_v = parse_bd(RLC_RESONANCE, r, "resistance")?;
    let l_v = parse_bd(RLC_RESONANCE, l, "inductance")?;
    let c_v = parse_bd(RLC_RESONANCE, c, "capacitance")?;
    positive(RLC_RESONANCE, &r_v, "resistance")?;
    positive(RLC_RESONANCE, &l_v, "inductance")?;
    positive(RLC_RESONANCE, &c_v, "capacitance")?;
    Ok(RlcInputs {
        r: r_v,
        l: l_v,
        c: c_v,
    })
}

fn rlc_resonant_frequency(l: &BigDecimal, c: &BigDecimal) -> BigDecimal {
    let lc = mul_ctx(l, c);
    let sqrt_lc = af_sqrt(&lc);
    let denom = mul_ctx(&TWO_PI, &sqrt_lc);
    div_scaled(&BigDecimal::from(1), &denom)
}

fn rlc_q_factor(r: &BigDecimal, l: &BigDecimal, c: &BigDecimal) -> BigDecimal {
    let l_over_c = div_scaled(l, c);
    let z_ratio = af_sqrt(&l_over_c);
    div_scaled(&z_ratio, r)
}

/// RLC resonant frequency, Q factor, bandwidth.
#[must_use]
pub fn rlc_resonance(r: &str, l: &str, c: &str) -> String {
    let inputs = match parse_rlc_positive(r, l, c) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let resonant_freq = rlc_resonant_frequency(&inputs.l, &inputs.c);
    let q_factor = rlc_q_factor(&inputs.r, &inputs.l, &inputs.c);
    if q_factor.is_zero() {
        return error(
            RLC_RESONANCE,
            ErrorCode::DivisionByZero,
            "Q factor must not be zero",
        );
    }
    let bandwidth = div_scaled(&resonant_freq, &q_factor);
    Response::ok(RLC_RESONANCE)
        .field("RESONANT_FREQUENCY", strip_plain(&resonant_freq))
        .field("Q_FACTOR", strip_plain(&q_factor))
        .field("BANDWIDTH", strip_plain(&bandwidth))
        .build()
}

struct ImpedanceInputs {
    r: BigDecimal,
    l: BigDecimal,
    c: BigDecimal,
    frequency: BigDecimal,
}

fn parse_impedance(r: &str, l: &str, c: &str, frequency: &str) -> Result<ImpedanceInputs, String> {
    let r_v = parse_bd(IMPEDANCE, r, "resistance")?;
    let l_v = parse_bd(IMPEDANCE, l, "inductance")?;
    let c_v = parse_bd(IMPEDANCE, c, "capacitance")?;
    let f = parse_bd(IMPEDANCE, frequency, "frequency")?;
    positive(IMPEDANCE, &f, "frequency")?;
    positive(IMPEDANCE, &c_v, "capacitance")?;
    if r_v.is_negative() {
        return Err(error_with_detail(
            IMPEDANCE,
            ErrorCode::InvalidInput,
            "resistance must not be negative",
            &format!("resistance={}", strip_plain(&r_v)),
        ));
    }
    if l_v.is_negative() {
        return Err(error_with_detail(
            IMPEDANCE,
            ErrorCode::InvalidInput,
            "inductance must not be negative",
            &format!("inductance={}", strip_plain(&l_v)),
        ));
    }
    Ok(ImpedanceInputs {
        r: r_v,
        l: l_v,
        c: c_v,
        frequency: f,
    })
}

fn series_reactance(omega: &BigDecimal, l: &BigDecimal, c: &BigDecimal) -> BigDecimal {
    let x_l = mul_ctx(omega, l);
    let omega_c = mul_ctx(omega, c);
    let x_c = div_scaled(&BigDecimal::from(1), &omega_c);
    sub_ctx(&x_l, &x_c)
}

/// Series RLC impedance magnitude + phase (degrees) + real + imag.
#[must_use]
pub fn impedance(r: &str, l: &str, c: &str, frequency: &str) -> String {
    let inputs = match parse_impedance(r, l, c, frequency) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let omega = mul_ctx(&TWO_PI, &inputs.frequency);
    let reactance = series_reactance(&omega, &inputs.l, &inputs.c);
    let r_squared = mul_ctx(&inputs.r, &inputs.r);
    let x_squared = mul_ctx(&reactance, &reactance);
    let magnitude = af_sqrt(&add_ctx(&r_squared, &x_squared));
    let phase_deg = af_atan2_degrees(&reactance, &inputs.r);
    Response::ok(IMPEDANCE)
        .field("MAGNITUDE", strip_plain(&magnitude))
        .field("PHASE_DEG", strip_plain(&phase_deg))
        .field("REAL", strip_plain(&inputs.r))
        .field("IMAG", strip_plain(&reactance))
        .build()
}

// --- Decibel ---

/// Decibel conversion: `powerToDb`, `voltageToDb`, `dbToPower`, `dbToVoltage`.
#[must_use]
pub fn decibel_convert(value: &str, mode: &str) -> String {
    let val = match parse_bd(DECIBEL_CONVERT, value, "value") {
        Ok(v) => v,
        Err(e) => return e,
    };
    match compute_decibel(&val, mode) {
        Ok(out) => Response::ok(DECIBEL_CONVERT)
            .result(strip_plain(&out))
            .build(),
        Err(e) => e,
    }
}

fn compute_decibel(val: &BigDecimal, mode: &str) -> Result<BigDecimal, String> {
    let ten = BigDecimal::from(10);
    let twenty = BigDecimal::from(20);
    match mode {
        "powerToDb" => {
            positive(DECIBEL_CONVERT, val, "value")?;
            Ok(mul_ctx(&ten, &af_log10(val)))
        }
        "voltageToDb" => {
            positive(DECIBEL_CONVERT, val, "value")?;
            Ok(mul_ctx(&twenty, &af_log10(val)))
        }
        "dbToPower" => {
            // `10^(value/10)` for `value ≥ ~1e5 dB` yields numbers astro-float
            // cannot represent inside its ~62-bit exponent window — the
            // `BigFloat::format` call then panics with "format failed" and
            // the stdio transport drops the connection. Bound the input
            // well below that ceiling; realistic decibel ranges top out
            // around ±200 dB anyway.
            reject_huge_db(val)?;
            let exponent = div_scaled(val, &ten);
            Ok(af_pow10(&exponent))
        }
        "dbToVoltage" => {
            reject_huge_db(val)?;
            let exponent = div_scaled(val, &twenty);
            Ok(af_pow10(&exponent))
        }
        _ => Err(error_with_detail(
            DECIBEL_CONVERT,
            ErrorCode::InvalidInput,
            "mode must be powerToDb, voltageToDb, dbToPower, or dbToVoltage",
            &format!("mode={mode}"),
        )),
    }
}

/// Upper bound on the absolute value of a decibel input before
/// `af_pow10(value/10)` trips astro-float's internal format/exponent limits
/// and panics. `10^10000` already overflows any practical numeric type, so
/// this ceiling is well above anything a caller can legitimately need.
const DB_MAX_ABS: i64 = 100_000;

fn reject_huge_db(val: &BigDecimal) -> Result<(), String> {
    let limit = BigDecimal::from(DB_MAX_ABS);
    if val.abs() > limit {
        return Err(error_with_detail(
            DECIBEL_CONVERT,
            ErrorCode::OutOfRange,
            "decibel magnitude exceeds the supported range",
            &format!("value={}, max_abs={}", strip_plain(val), DB_MAX_ABS,),
        ));
    }
    Ok(())
}

// --- Filter cutoff ---

/// RC filter cutoff frequency. fc = 1 / (2π R C).
#[must_use]
pub fn filter_cutoff(resistance: &str, reactive: &str, filter_type: &str) -> String {
    let r = match parse_bd(FILTER_CUTOFF, resistance, "resistance") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let c = match parse_bd(FILTER_CUTOFF, reactive, "capacitance") {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = positive(FILTER_CUTOFF, &r, "resistance") {
        return e;
    }
    if let Err(e) = positive(FILTER_CUTOFF, &c, "capacitance") {
        return e;
    }
    let rc = mul_ctx(&r, &c);
    let denom = mul_ctx(&TWO_PI, &rc);
    let freq = div_scaled(&BigDecimal::from(1), &denom);
    let ftype = match validate_filter_type(FILTER_CUTOFF, filter_type) {
        Ok(v) => v,
        Err(e) => return e,
    };
    Response::ok(FILTER_CUTOFF)
        .field("CUTOFF_HZ", strip_plain(&freq))
        .field("FILTER_TYPE", ftype)
        .build()
}

// --- LED / Wheatstone ---

/// LED current-limiting resistor: R = (Vs - Vf) / If.
#[must_use]
pub fn led_resistor(vs: &str, vf: &str, i_f: &str) -> String {
    let supply = match parse_bd(LED_RESISTOR, vs, "supply_voltage") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let forward = match parse_bd(LED_RESISTOR, vf, "forward_voltage") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let if_v = match parse_bd(LED_RESISTOR, i_f, "forward_current") {
        Ok(v) => v,
        Err(e) => return e,
    };
    if supply.is_negative() {
        return error_with_detail(
            LED_RESISTOR,
            ErrorCode::InvalidInput,
            "supply voltage must not be negative",
            &format!("supply_voltage={}", strip_plain(&supply)),
        );
    }
    if forward.is_negative() {
        return error_with_detail(
            LED_RESISTOR,
            ErrorCode::InvalidInput,
            "forward voltage must not be negative",
            &format!("forward_voltage={}", strip_plain(&forward)),
        );
    }
    if supply <= forward {
        return error(
            LED_RESISTOR,
            ErrorCode::InvalidInput,
            "supply voltage must be greater than forward voltage",
        );
    }
    if if_v.is_zero() || if_v.is_negative() {
        return error(
            LED_RESISTOR,
            ErrorCode::InvalidInput,
            "forward current must be greater than zero",
        );
    }
    let diff = sub_ctx(&supply, &forward);
    let resistance = div_scaled(&diff, &if_v);
    let power = mul_ctx(&diff, &if_v);
    Response::ok(LED_RESISTOR)
        .field("RESISTANCE", strip_plain(&resistance))
        .field("POWER_DISSIPATED", strip_plain(&power))
        .build()
}

/// Wheatstone bridge balance resistor: R4 = R3 * R2 / R1.
#[must_use]
pub fn wheatstone_bridge(r1: &str, r2: &str, r3: &str) -> String {
    let r1_v = match parse_bd(WHEATSTONE_BRIDGE, r1, "r1") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let r2_v = match parse_bd(WHEATSTONE_BRIDGE, r2, "r2") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let r3_v = match parse_bd(WHEATSTONE_BRIDGE, r3, "r3") {
        Ok(v) => v,
        Err(e) => return e,
    };
    if r1_v.is_zero() || r1_v.is_negative() {
        return error_with_detail(
            WHEATSTONE_BRIDGE,
            ErrorCode::InvalidInput,
            "r1 must be positive",
            &format!("r1={r1}"),
        );
    }
    if r2_v.is_negative() {
        return error_with_detail(
            WHEATSTONE_BRIDGE,
            ErrorCode::InvalidInput,
            "r2 must not be negative",
            &format!("r2={r2}"),
        );
    }
    if r3_v.is_negative() {
        return error_with_detail(
            WHEATSTONE_BRIDGE,
            ErrorCode::InvalidInput,
            "r3 must not be negative",
            &format!("r3={r3}"),
        );
    }
    let r4 = div_scaled(&mul_ctx(&r3_v, &r2_v), &r1_v);
    Response::ok(WHEATSTONE_BRIDGE)
        .result(strip_plain(&r4))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ohms_law_vi_produces_r_and_p() {
        assert_eq!(
            ohms_law("12", "2", "", ""),
            "OHMS_LAW: OK | VOLTAGE: 12 | CURRENT: 2 | RESISTANCE: 6 | POWER: 24"
        );
    }

    #[test]
    fn ohms_law_vr_produces_i_and_p() {
        assert_eq!(
            ohms_law("10", "", "5", ""),
            "OHMS_LAW: OK | VOLTAGE: 10 | CURRENT: 2 | RESISTANCE: 5 | POWER: 20"
        );
    }

    #[test]
    fn ohms_law_rp_uses_sqrt_path() {
        // R=4, P=100 → V=sqrt(400)=20, I=sqrt(25)=5
        assert_eq!(
            ohms_law("", "", "4", "100"),
            "OHMS_LAW: OK | VOLTAGE: 20 | CURRENT: 5 | RESISTANCE: 4 | POWER: 100"
        );
    }

    #[test]
    fn ohms_law_wrong_count_errors() {
        assert_eq!(
            ohms_law("1", "2", "3", ""),
            "OHMS_LAW: ERROR\nREASON: [INVALID_INPUT] exactly two of V, I, R, P must be provided"
        );
        assert_eq!(
            ohms_law("", "", "", ""),
            "OHMS_LAW: ERROR\nREASON: [INVALID_INPUT] exactly two of V, I, R, P must be provided"
        );
    }

    #[test]
    fn resistor_series_sums() {
        assert_eq!(
            resistor_combination("10, 20, 30", "series"),
            "RESISTOR_COMBINATION: OK | RESULT: 60"
        );
    }

    #[test]
    fn resistor_parallel_halves_equal_pair() {
        assert_eq!(
            resistor_combination("10, 10", "parallel"),
            "RESISTOR_COMBINATION: OK | RESULT: 5"
        );
    }

    #[test]
    fn capacitor_parallel_sums() {
        assert_eq!(
            capacitor_combination("1e-6, 2e-6, 3e-6", "parallel"),
            "CAPACITOR_COMBINATION: OK | RESULT: 0.000006"
        );
    }

    #[test]
    fn capacitor_series_reciprocal() {
        assert_eq!(
            capacitor_combination("2e-6, 2e-6", "series"),
            "CAPACITOR_COMBINATION: OK | RESULT: 0.000001"
        );
    }

    #[test]
    fn inductor_series_sums() {
        assert_eq!(
            inductor_combination("0.001, 0.002", "series"),
            "INDUCTOR_COMBINATION: OK | RESULT: 0.003"
        );
    }

    #[test]
    fn combination_bad_mode_errors() {
        assert_eq!(
            resistor_combination("10, 10", "weird"),
            "RESISTOR_COMBINATION: ERROR\nREASON: [INVALID_INPUT] mode must be 'series' or 'parallel'\nDETAIL: mode=weird"
        );
    }

    #[test]
    fn voltage_divider_half() {
        assert_eq!(
            voltage_divider("5", "1000", "1000"),
            "VOLTAGE_DIVIDER: OK | VOUT: 2.5"
        );
    }

    #[test]
    fn current_divider_equal_split() {
        assert_eq!(
            current_divider("2", "1000", "1000"),
            "CURRENT_DIVIDER: OK | I1: 1 | I2: 1"
        );
    }

    #[test]
    fn rc_time_constant_millis() {
        // 1kΩ × 1µF = 1ms, fc = 1/(2π·1e-3) ≈ 159.1549
        let out = rc_time_constant("1000", "0.000001");
        assert!(
            out.starts_with("RC_TIME_CONSTANT: OK | TAU: 0.001 | CUTOFF_FREQUENCY: 159.15494309"),
            "got: {out}"
        );
    }

    #[test]
    fn rl_time_constant_basic() {
        // L=1H, R=1kΩ → τ = 0.001, fc = R/(2π·L) = 1000/(2π) ≈ 159.154943...
        let out = rl_time_constant("1000", "1");
        assert!(
            out.starts_with("RL_TIME_CONSTANT: OK | TAU: 0.001 | CUTOFF_FREQUENCY: 159.15494309"),
            "got: {out}"
        );
    }

    #[test]
    fn led_resistor_standard() {
        // 5V supply, 2V Vf, 20mA If → 150Ω, P = 3*0.02 = 0.06W
        assert_eq!(
            led_resistor("5", "2", "0.02"),
            "LED_RESISTOR: OK | RESISTANCE: 150 | POWER_DISSIPATED: 0.06"
        );
    }

    #[test]
    fn led_resistor_bad_voltages() {
        assert_eq!(
            led_resistor("2", "5", "0.01"),
            "LED_RESISTOR: ERROR\nREASON: [INVALID_INPUT] supply voltage must be greater than forward voltage"
        );
    }

    #[test]
    fn led_resistor_bad_current() {
        assert_eq!(
            led_resistor("5", "2", "0"),
            "LED_RESISTOR: ERROR\nREASON: [INVALID_INPUT] forward current must be greater than zero"
        );
    }

    #[test]
    fn wheatstone_basic() {
        // R1=100, R2=200, R3=300 → R4 = 300*200/100 = 600
        assert_eq!(
            wheatstone_bridge("100", "200", "300"),
            "WHEATSTONE_BRIDGE: OK | RESULT: 600"
        );
    }

    #[test]
    fn wheatstone_zero_denominator() {
        assert_eq!(
            wheatstone_bridge("0", "10", "10"),
            "WHEATSTONE_BRIDGE: ERROR\nREASON: [INVALID_INPUT] r1 must be positive\nDETAIL: r1=0"
        );
    }

    #[test]
    fn decibel_power_to_db() {
        // 10*log10(100) = 20
        assert_eq!(
            decibel_convert("100", "powerToDb"),
            "DECIBEL_CONVERT: OK | RESULT: 20"
        );
    }

    #[test]
    fn decibel_voltage_to_db() {
        // 20*log10(10) = 20
        assert_eq!(
            decibel_convert("10", "voltageToDb"),
            "DECIBEL_CONVERT: OK | RESULT: 20"
        );
    }

    #[test]
    fn decibel_bad_mode() {
        assert_eq!(
            decibel_convert("1", "oops"),
            "DECIBEL_CONVERT: ERROR\nREASON: [INVALID_INPUT] mode must be powerToDb, voltageToDb, dbToPower, or dbToVoltage\nDETAIL: mode=oops"
        );
    }

    #[test]
    fn decibel_negative_power_error() {
        assert_eq!(
            decibel_convert("-1", "powerToDb"),
            "DECIBEL_CONVERT: ERROR\nREASON: [INVALID_INPUT] value must be positive\nDETAIL: value=-1"
        );
    }

    #[test]
    fn rc_time_constant_rejects_negative_resistance() {
        // Regression: previously produced TAU: -0.001 silently.
        assert_eq!(
            rc_time_constant("-1000", "0.000001"),
            "RC_TIME_CONSTANT: ERROR\nREASON: [INVALID_INPUT] resistance must be positive\nDETAIL: resistance=-1000"
        );
    }

    #[test]
    fn rc_time_constant_rejects_negative_capacitance() {
        assert_eq!(
            rc_time_constant("1000", "-0.000001"),
            "RC_TIME_CONSTANT: ERROR\nREASON: [INVALID_INPUT] capacitance must be positive\nDETAIL: capacitance=-0.000001"
        );
    }

    #[test]
    fn rl_time_constant_rejects_negative_resistance() {
        assert_eq!(
            rl_time_constant("-10", "0.001"),
            "RL_TIME_CONSTANT: ERROR\nREASON: [INVALID_INPUT] resistance must be positive\nDETAIL: resistance=-10"
        );
    }

    #[test]
    fn filter_cutoff_rejects_negative_reactive() {
        assert_eq!(
            filter_cutoff("1000", "-0.000001", "lowpass"),
            "FILTER_CUTOFF: ERROR\nREASON: [INVALID_INPUT] capacitance must be positive\nDETAIL: capacitance=-0.000001"
        );
    }

    #[test]
    fn filter_cutoff_prefix_and_type() {
        let out = filter_cutoff("1000", "0.000001", "LowPass");
        assert!(
            out.starts_with("FILTER_CUTOFF: OK | CUTOFF_HZ: 159.15494309"),
            "got: {out}"
        );
        assert!(out.ends_with(" | FILTER_TYPE: lowpass"), "got: {out}");
    }

    #[test]
    fn filter_cutoff_bad_type_errors() {
        assert_eq!(
            filter_cutoff("1000", "0.000001", "bandstop"),
            "FILTER_CUTOFF: ERROR\nREASON: [INVALID_INPUT] filter type must be 'lowpass' or 'highpass'\nDETAIL: filter=bandstop"
        );
    }

    #[test]
    fn impedance_rejects_negative_frequency() {
        assert_eq!(
            impedance("10", "0.001", "0.000001", "-1000"),
            "IMPEDANCE: ERROR\nREASON: [INVALID_INPUT] frequency must be positive\nDETAIL: frequency=-1000"
        );
    }

    #[test]
    fn impedance_rejects_zero_frequency() {
        assert_eq!(
            impedance("10", "0.001", "0.000001", "0"),
            "IMPEDANCE: ERROR\nREASON: [INVALID_INPUT] frequency must be positive\nDETAIL: frequency=0"
        );
    }

    #[test]
    fn impedance_rejects_negative_capacitance() {
        assert_eq!(
            impedance("10", "0.001", "-0.000001", "100"),
            "IMPEDANCE: ERROR\nREASON: [INVALID_INPUT] capacitance must be positive\nDETAIL: capacitance=-0.000001"
        );
    }

    #[test]
    fn impedance_near_resonance_magnitude_matches_r() {
        // Pick R=10, L=1e-3, C=1e-6 — at f ≈ 5032.92 Hz, X_L=X_C → mag=R=10, phase≈0
        let out = impedance("10", "0.001", "0.000001", "5032.9216");
        assert!(out.starts_with("IMPEDANCE: OK | MAGNITUDE: "), "got: {out}");
        // Extract MAGNITUDE token.
        let after = out.strip_prefix("IMPEDANCE: OK | MAGNITUDE: ").unwrap();
        let end = after.find(" | ").unwrap();
        let mag: f64 = after[..end].parse().unwrap();
        assert!((mag - 10.0).abs() < 0.05, "mag was {mag}");
        assert!(out.contains(" | PHASE_DEG: "), "got: {out}");
        assert!(out.contains(" | REAL: 10 | IMAG: "), "got: {out}");
    }

    #[test]
    fn rlc_resonance_produces_all_fields() {
        let out = rlc_resonance("10", "0.001", "0.000001");
        assert!(out.starts_with("RLC_RESONANCE: OK | RESONANT_FREQUENCY: "));
        assert!(out.contains(" | Q_FACTOR: "));
        assert!(out.contains(" | BANDWIDTH: "));
    }

    #[test]
    fn voltage_divider_rejects_negative_r1() {
        let out = voltage_divider("10", "-100", "50");
        assert!(out.contains("VOLTAGE_DIVIDER: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("r1 must be positive"));
    }

    #[test]
    fn voltage_divider_rejects_negative_r2() {
        let out = voltage_divider("10", "100", "-50");
        assert!(out.contains("VOLTAGE_DIVIDER: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("r2 must be positive"));
    }

    #[test]
    fn voltage_divider_rejects_zero_r1() {
        // Regression: previously silently returned Vout=Vin with R1=0 (short).
        let out = voltage_divider("5", "0", "1000");
        assert!(out.contains("VOLTAGE_DIVIDER: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("r1 must be positive"));
    }

    #[test]
    fn voltage_divider_rejects_zero_r2() {
        let out = voltage_divider("5", "1000", "0");
        assert!(out.contains("VOLTAGE_DIVIDER: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("r2 must be positive"));
    }

    #[test]
    fn current_divider_rejects_negative_r1() {
        let out = current_divider("5", "-100", "50");
        assert!(out.contains("CURRENT_DIVIDER: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("r1 must be positive"));
    }

    #[test]
    fn current_divider_rejects_negative_r2() {
        let out = current_divider("5", "100", "-50");
        assert!(out.contains("CURRENT_DIVIDER: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("r2 must be positive"));
    }

    #[test]
    fn current_divider_rejects_zero_r1() {
        // Regression: previously returned I1=Itotal, I2=0 with R1=0 (short).
        let out = current_divider("1", "0", "1000");
        assert!(out.contains("CURRENT_DIVIDER: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("r1 must be positive"));
    }

    #[test]
    fn current_divider_rejects_zero_r2() {
        let out = current_divider("1", "1000", "0");
        assert!(out.contains("CURRENT_DIVIDER: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("r2 must be positive"));
    }

    #[test]
    fn ohms_law_rejects_negative_voltage_vi() {
        // Regression: V=-5, I=1 used to produce R=-5, P=-5 (non-physical).
        let out = ohms_law("-5", "1", "", "");
        assert!(out.contains("OHMS_LAW: ERROR"));
        assert!(out.contains("voltage must not be negative"));
    }

    #[test]
    fn ohms_law_rejects_negative_current_ir() {
        let out = ohms_law("", "-2", "10", "");
        assert!(out.contains("OHMS_LAW: ERROR"));
        assert!(out.contains("current must not be negative"));
    }

    #[test]
    fn ohms_law_rejects_negative_resistance_vr() {
        let out = ohms_law("5", "", "-10", "");
        assert!(out.contains("OHMS_LAW: ERROR"));
        assert!(out.contains("resistance must not be negative"));
    }

    #[test]
    fn ohms_law_rejects_negative_power_rp() {
        let out = ohms_law("", "", "10", "-100");
        assert!(out.contains("OHMS_LAW: ERROR"));
        assert!(out.contains("power must not be negative"));
    }

    #[test]
    fn resistor_series_rejects_negative() {
        // Regression: series mode used to accept [-100, 200, 300] → 400 silently.
        let out = resistor_combination("-100, 200, 300", "series");
        assert!(out.contains("RESISTOR_COMBINATION: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("must not be negative"));
    }

    #[test]
    fn inductor_series_rejects_negative() {
        let out = inductor_combination("-0.001, 0.002", "series");
        assert!(out.contains("INDUCTOR_COMBINATION: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("must not be negative"));
    }

    #[test]
    fn capacitor_parallel_rejects_negative() {
        // For capacitors, parallel is the additive mode (reversed from R/L).
        let out = capacitor_combination("-1e-6, 2e-6", "parallel");
        assert!(out.contains("CAPACITOR_COMBINATION: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("must not be negative"));
    }

    #[test]
    fn led_resistor_rejects_negative_vf() {
        // Regression: Vf=-1.5 used to compute R=325 silently (non-physical).
        let out = led_resistor("5", "-1.5", "0.02");
        assert!(out.contains("LED_RESISTOR: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("forward voltage must not be negative"));
    }

    #[test]
    fn led_resistor_rejects_negative_vs() {
        let out = led_resistor("-5", "2", "0.02");
        assert!(out.contains("LED_RESISTOR: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("supply voltage must not be negative"));
    }

    #[test]
    fn rlc_resonance_rejects_negative_resistance() {
        let out = rlc_resonance("-10", "0.001", "0.000001");
        assert!(out.contains("RLC_RESONANCE: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("resistance must be positive"));
    }

    #[test]
    fn rlc_resonance_rejects_zero_inductance() {
        let out = rlc_resonance("10", "0", "0.000001");
        assert!(out.contains("RLC_RESONANCE: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("inductance must be positive"));
    }

    #[test]
    fn rlc_resonance_rejects_negative_capacitance() {
        let out = rlc_resonance("10", "0.001", "-0.000001");
        assert!(out.contains("RLC_RESONANCE: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("capacitance must be positive"));
    }

    #[test]
    fn wheatstone_bridge_rejects_negative_r1() {
        let out = wheatstone_bridge("-100", "200", "150");
        assert!(out.contains("WHEATSTONE_BRIDGE: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("r1 must be positive"));
    }

    #[test]
    fn wheatstone_bridge_rejects_negative_r2() {
        let out = wheatstone_bridge("100", "-200", "150");
        assert!(out.contains("WHEATSTONE_BRIDGE: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("r2 must not be negative"));
    }

    #[test]
    fn wheatstone_bridge_rejects_negative_r3() {
        let out = wheatstone_bridge("100", "200", "-150");
        assert!(out.contains("WHEATSTONE_BRIDGE: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("r3 must not be negative"));
    }

    #[test]
    fn voltage_divider_rejects_negative_vin() {
        // Regression: Vin=-12 used to silently produce Vout=-6 V.
        let out = voltage_divider("-12", "1000", "1000");
        assert!(out.contains("VOLTAGE_DIVIDER: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("vin must not be negative"));
    }

    #[test]
    fn voltage_divider_accepts_zero_vin() {
        // Zero input voltage is still physically valid — output is zero.
        let out = voltage_divider("0", "1000", "1000");
        assert!(out.contains("VOLTAGE_DIVIDER: OK"));
        assert!(out.contains("VOUT: 0"));
    }

    #[test]
    fn current_divider_rejects_negative_total_current() {
        // Regression: I_total=-1 used to produce I1=-0.5, I2=-0.5 silently.
        let out = current_divider("-1", "1000", "1000");
        assert!(out.contains("CURRENT_DIVIDER: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("totalCurrent must not be negative"));
    }

    #[test]
    fn current_divider_accepts_zero_total_current() {
        let out = current_divider("0", "1000", "1000");
        assert!(out.contains("CURRENT_DIVIDER: OK"));
    }
}
