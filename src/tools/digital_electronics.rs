//! Digital electronics tooling — base conversion, two's complement, Gray code,
//! bitwise ops, ADC/DAC, 555-timer, frequency↔period, Nyquist.
//!
//! All public functions return `String` via the shared response envelope
//! (`crate::mcp::message`). Success responses are inline; errors use the
//! three-line block form.

use std::num::NonZeroU64;
use std::str::FromStr;

use bigdecimal::{BigDecimal, RoundingMode};
use num_bigint::BigInt;
use num_traits::{Num, One, Signed, Zero};

use crate::engine::bigdecimal_ext::{
    DECIMAL128_PRECISION, DIVISION_SCALE, LN2_RECIPROCAL, strip_plain,
};
use crate::mcp::message::{ErrorCode, Response, error, error_with_detail};

// ------------------------------------------------------------------ //
//  Tool names
// ------------------------------------------------------------------ //

const CONVERT_BASE: &str = "CONVERT_BASE";
const TWOS_COMPLEMENT: &str = "TWOS_COMPLEMENT";
const GRAY_CODE: &str = "GRAY_CODE";
const BITWISE_OP: &str = "BITWISE_OP";
const ADC_RESOLUTION: &str = "ADC_RESOLUTION";
const DAC_OUTPUT: &str = "DAC_OUTPUT";
const TIMER_555_ASTABLE: &str = "TIMER_555_ASTABLE";
const TIMER_555_MONOSTABLE: &str = "TIMER_555_MONOSTABLE";
const FREQUENCY_PERIOD: &str = "FREQUENCY_PERIOD";
const NYQUIST_RATE: &str = "NYQUIST_RATE";

const TO_TWOS: &str = "toTwos";
const FROM_TWOS: &str = "fromTwos";
const TO_GRAY: &str = "toGray";
const FROM_GRAY: &str = "fromGray";
const MIN_BASE: i32 = 2;
const MAX_BASE: i32 = 36;
const MAX_BITS: i32 = 64;

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

fn pow2(bits: u32) -> BigDecimal {
    BigDecimal::from(BigInt::one() << bits)
}

fn check_base(tool: &str, base: i32) -> Result<(), String> {
    if (MIN_BASE..=MAX_BASE).contains(&base) {
        Ok(())
    } else {
        Err(error_with_detail(
            tool,
            ErrorCode::OutOfRange,
            &format!("base must be between {MIN_BASE} and {MAX_BASE}"),
            &format!("base={base}"),
        ))
    }
}

fn check_bit_width(tool: &str, bits: i32) -> Result<(), String> {
    if (1..=MAX_BITS).contains(&bits) {
        Ok(())
    } else {
        Err(error_with_detail(
            tool,
            ErrorCode::OutOfRange,
            &format!("bit width must be between 1 and {MAX_BITS}"),
            &format!("bits={bits}"),
        ))
    }
}

fn pad_binary(binary: &str, width: usize) -> String {
    if binary.len() >= width {
        binary.to_string()
    } else {
        let mut out = String::with_capacity(width);
        for _ in 0..(width - binary.len()) {
            out.push('0');
        }
        out.push_str(binary);
        out
    }
}

/// Convert between any two bases (2..=36).
#[must_use]
pub fn convert_base(value: &str, from_base: i32, to_base: i32) -> String {
    if let Err(e) = check_base(CONVERT_BASE, from_base) {
        return e;
    }
    if let Err(e) = check_base(CONVERT_BASE, to_base) {
        return e;
    }
    // `from_base` and `to_base` are validated into 2..=36 by `check_base`, so
    // the `i32 -> u32` cast is safe and non-lossy.
    let from_base_u = from_base.cast_unsigned();
    let to_base_u = to_base.cast_unsigned();
    BigInt::from_str_radix(value.trim(), from_base_u).map_or_else(
        |_| {
            error_with_detail(
                CONVERT_BASE,
                ErrorCode::ParseError,
                &format!("invalid number for base {from_base}"),
                &format!("value={value}"),
            )
        },
        |big| {
            Response::ok(CONVERT_BASE)
                .result(big.to_str_radix(to_base_u).to_uppercase())
                .build()
        },
    )
}

/// Two's-complement encode (`toTwos`) or decode (`fromTwos`).
#[must_use]
pub fn twos_complement(value: &str, bits: i32, direction: &str) -> String {
    if let Err(e) = check_bit_width(TWOS_COMPLEMENT, bits) {
        return e;
    }
    // `bits` is validated into 1..=64 by `check_bit_width`, so the cast is safe.
    let bits_u = bits.cast_unsigned();
    match direction {
        TO_TWOS => encode_to_twos(value, bits_u),
        FROM_TWOS => decode_from_twos(value, bits_u),
        _ => error_with_detail(
            TWOS_COMPLEMENT,
            ErrorCode::InvalidInput,
            &format!("direction must be '{TO_TWOS}' or '{FROM_TWOS}'"),
            &format!("direction={direction}"),
        ),
    }
}

fn encode_to_twos(value: &str, bits: u32) -> String {
    let Ok(parsed) = BigInt::from_str(value.trim()) else {
        return error_with_detail(
            TWOS_COMPLEMENT,
            ErrorCode::ParseError,
            "value is not a valid integer",
            &format!("value={value}"),
        );
    };
    // Reject values outside the signed range; the old implementation silently
    // truncated via `value & mask`, so `toTwos(1024, bits=8)` returned
    // `00000000` instead of an error.
    let max: BigInt = (BigInt::one() << (bits - 1)) - BigInt::one();
    let min: BigInt = -(BigInt::one() << (bits - 1));
    if parsed > max || parsed < min {
        return error_with_detail(
            TWOS_COMPLEMENT,
            ErrorCode::OutOfRange,
            &format!("value is outside the {bits}-bit signed range"),
            &format!(
                "value={value}, min={}, max={}",
                min.to_str_radix(10),
                max.to_str_radix(10)
            ),
        );
    }
    let mask: BigInt = (BigInt::one() << bits) - BigInt::one();
    let twos = parsed & mask;
    let encoded = pad_binary(&twos.to_str_radix(2), bits as usize);
    Response::ok(TWOS_COMPLEMENT).result(encoded).build()
}

fn decode_from_twos(value: &str, bits: u32) -> String {
    let trimmed = value.trim();
    // Reject strings longer than the declared bit width: decoding
    // `111111111` (9 bits) against `bits=8` used to silently yield 511,
    // which isn't expressible in any 8-bit signed encoding. Shorter
    // strings are still valid — they encode non-negative values whose
    // top bits default to 0.
    if trimmed.len() > bits as usize {
        return error_with_detail(
            TWOS_COMPLEMENT,
            ErrorCode::OutOfRange,
            &format!("binary string is longer than {bits}-bit width"),
            &format!("value={value}, length={}, bits={bits}", trimmed.len()),
        );
    }
    let Ok(parsed) = BigInt::from_str_radix(trimmed, 2) else {
        return error_with_detail(
            TWOS_COMPLEMENT,
            ErrorCode::ParseError,
            "value is not a valid binary string",
            &format!("value={value}"),
        );
    };
    let msb_set = trimmed.starts_with('1') && trimmed.len() == bits as usize;
    let result = if msb_set {
        parsed - (BigInt::one() << bits)
    } else {
        parsed
    };
    Response::ok(TWOS_COMPLEMENT)
        .result(result.to_string())
        .build()
}

/// Gray-code encode (`toGray`) or decode (`fromGray`).
#[must_use]
pub fn gray_code(value: &str, direction: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c != '0' && c != '1') {
        return error_with_detail(
            GRAY_CODE,
            ErrorCode::ParseError,
            "value is not a valid binary string",
            &format!("value={value}"),
        );
    }
    let width = trimmed.len();
    let encoded = match direction {
        TO_GRAY => encode_binary_to_gray(trimmed, width),
        FROM_GRAY => decode_gray_to_binary(trimmed, width),
        _ => {
            return error_with_detail(
                GRAY_CODE,
                ErrorCode::InvalidInput,
                &format!("direction must be '{TO_GRAY}' or '{FROM_GRAY}'"),
                &format!("direction={direction}"),
            );
        }
    };
    Response::ok(GRAY_CODE).result(encoded).build()
}

fn encode_binary_to_gray(binary: &str, width: usize) -> String {
    let num: BigInt = BigInt::from_str_radix(binary, 2).expect("validated binary");
    let shifted: BigInt = &num >> 1_u32;
    let gray: BigInt = &num ^ &shifted;
    pad_binary(&gray.to_str_radix(2), width)
}

fn decode_gray_to_binary(binary: &str, width: usize) -> String {
    let mut num: BigInt = BigInt::from_str_radix(binary, 2).expect("validated binary");
    let mut mask: BigInt = &num >> 1_u32;
    while !mask.is_zero() {
        num = &num ^ &mask;
        mask >>= 1_u32;
    }
    pad_binary(&num.to_str_radix(2), width)
}

/// Peel a radix prefix (`0x`/`0X`, `0b`/`0B`, `0o`/`0O`) off `body` and return
/// `(radix, digit-tail)`. Falls back to `(10, body)` when no prefix is present.
fn detect_radix(body: &str) -> (u32, &str) {
    const PREFIXES: &[(&str, u32)] = &[
        ("0x", 16),
        ("0X", 16),
        ("0b", 2),
        ("0B", 2),
        ("0o", 8),
        ("0O", 8),
    ];
    for (pfx, radix) in PREFIXES {
        if let Some(rest) = body.strip_prefix(pfx) {
            return (*radix, rest);
        }
    }
    (10, body)
}

/// Parse an integer literal in decimal (default), hex (`0x`/`-0x`), octal
/// (`0o`), or binary (`0b`). Matches common electronics-tool notation so
/// `bitwiseOp("0xFF", "0x0F", "XOR")` just works.
fn parse_bitwise_operand(raw: &str) -> Option<BigInt> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (negative, body) = match trimmed.as_bytes().first() {
        Some(b'-') => (true, &trimmed[1..]),
        Some(b'+') => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    let (radix, digits) = detect_radix(body);
    if digits.is_empty() {
        return None;
    }
    let magnitude = BigInt::parse_bytes(digits.as_bytes(), radix)?;
    Some(if negative { -magnitude } else { magnitude })
}

fn parse_shift_amount(raw: &str) -> Option<u32> {
    let value = parse_bitwise_operand(raw)?;
    u32::try_from(value).ok()
}

/// Bitwise AND/OR/XOR/NOT/SHL/SHR. Returns the decimal result.
#[must_use]
pub fn bitwise_op(a: &str, b: &str, operation: &str) -> String {
    let Some(val_a) = parse_bitwise_operand(a) else {
        return error_with_detail(
            BITWISE_OP,
            ErrorCode::ParseError,
            "operand A is not a valid integer",
            &format!("a={a}"),
        );
    };
    let op = operation.to_ascii_uppercase();
    let computed: Option<BigInt> = match op.as_str() {
        "AND" => parse_bitwise_operand(b).map(|vb| &val_a & &vb),
        "OR" => parse_bitwise_operand(b).map(|vb| &val_a | &vb),
        "XOR" => parse_bitwise_operand(b).map(|vb| &val_a ^ &vb),
        "NOT" => Some(!val_a),
        "SHL" => parse_shift_amount(b).map(|shift| &val_a << shift),
        "SHR" => parse_shift_amount(b).map(|shift| &val_a >> shift),
        _ => {
            return error_with_detail(
                BITWISE_OP,
                ErrorCode::InvalidInput,
                "unknown operation",
                &format!("operation={operation}"),
            );
        }
    };
    computed.map_or_else(
        || {
            error_with_detail(
                BITWISE_OP,
                ErrorCode::ParseError,
                "operand B is not a valid integer",
                &format!("b={b}"),
            )
        },
        |value| Response::ok(BITWISE_OP).result(value.to_string()).build(),
    )
}

/// ADC resolution: `lsb = Vref / 2^bits`, `stepCount = 2^bits - 1`.
#[must_use]
pub fn adc_resolution(bits: i32, vref: &str) -> String {
    if let Err(e) = check_bit_width(ADC_RESOLUTION, bits) {
        return e;
    }
    let vref_v = match parse_bd(ADC_RESOLUTION, vref, "vref") {
        Ok(v) => v,
        Err(e) => return e,
    };
    // `bits` is validated into 1..=64 by `check_bit_width`, so the cast is safe.
    let levels = pow2(bits.cast_unsigned());
    let lsb = div_scaled(&vref_v, &levels);
    let step_count = sub_ctx(&levels, &BigDecimal::from(1));
    Response::ok(ADC_RESOLUTION)
        .field("BITS", bits.to_string())
        .field("LSB", strip_plain(&lsb))
        .field("STEP_COUNT", strip_plain(&step_count))
        .build()
}

/// DAC output voltage: Vout = Vref * code / 2^bits.
#[must_use]
pub fn dac_output(bits: i32, vref: &str, code: i64) -> String {
    if let Err(e) = check_bit_width(DAC_OUTPUT, bits) {
        return e;
    }
    let max_code: i128 = if bits == 64 {
        i128::from(i64::MAX)
    } else {
        (1_i128 << bits) - 1
    };
    if i128::from(code) < 0 || i128::from(code) > max_code {
        return error_with_detail(
            DAC_OUTPUT,
            ErrorCode::OutOfRange,
            &format!("code must be between 0 and {max_code}"),
            &format!("code={code}"),
        );
    }
    let vref_v = match parse_bd(DAC_OUTPUT, vref, "vref") {
        Ok(v) => v,
        Err(e) => return e,
    };
    // `bits` is validated into 1..=64 by `check_bit_width`, so the cast is safe.
    let levels = pow2(bits.cast_unsigned());
    let vout = div_scaled(&mul_ctx(&vref_v, &BigDecimal::from(code)), &levels);
    Response::ok(DAC_OUTPUT)
        .field("BITS", bits.to_string())
        .field("CODE", code.to_string())
        .field("VOUT", strip_plain(&vout))
        .build()
}

/// 555-timer astable output: frequency, period, dutyCycle (%), high/low time.
///
struct TimerAstableInputs {
    r1: BigDecimal,
    r2: BigDecimal,
    c: BigDecimal,
}

fn parse_timer_astable(r1: &str, r2: &str, c: &str) -> Result<TimerAstableInputs, String> {
    let r1_v = parse_bd(TIMER_555_ASTABLE, r1, "r1")?;
    let r2_v = parse_bd(TIMER_555_ASTABLE, r2, "r2")?;
    let c_v = parse_bd(TIMER_555_ASTABLE, c, "c")?;
    positive_bd(TIMER_555_ASTABLE, &r1_v, "r1")?;
    positive_bd(TIMER_555_ASTABLE, &r2_v, "r2")?;
    positive_bd(TIMER_555_ASTABLE, &c_v, "c")?;
    Ok(TimerAstableInputs {
        r1: r1_v,
        r2: r2_v,
        c: c_v,
    })
}

struct TimerAstableTiming {
    freq: BigDecimal,
    period: BigDecimal,
    duty: BigDecimal,
    high_time: BigDecimal,
    low_time: BigDecimal,
}

fn compute_timer_astable(inputs: &TimerAstableInputs) -> Result<TimerAstableTiming, String> {
    let two = BigDecimal::from(2);
    let r1_plus_2r2 = add_ctx(&inputs.r1, &mul_ctx(&inputs.r2, &two));
    let denominator = mul_ctx(&r1_plus_2r2, &inputs.c);
    if denominator.is_zero() {
        return Err(error(
            TIMER_555_ASTABLE,
            ErrorCode::DivisionByZero,
            "(R1 + 2·R2)·C must not be zero",
        ));
    }
    let freq = div_scaled(&LN2_RECIPROCAL, &denominator);
    if freq.is_zero() {
        return Err(error(
            TIMER_555_ASTABLE,
            ErrorCode::DivisionByZero,
            "frequency must not be zero",
        ));
    }
    let period = div_scaled(&BigDecimal::from(1), &freq);
    let r_sum = add_ctx(&inputs.r1, &inputs.r2);
    let duty = mul_ctx(&div_scaled(&r_sum, &r1_plus_2r2), &BigDecimal::from(100));
    let ln2_literal = BigDecimal::from_str("0.69314718055994530941723212145817656808")
        .expect("valid ln(2) literal");
    let high_time = mul_ctx(&mul_ctx(&ln2_literal, &r_sum), &inputs.c);
    let low_time = mul_ctx(&mul_ctx(&ln2_literal, &inputs.r2), &inputs.c);
    Ok(TimerAstableTiming {
        freq,
        period,
        duty,
        high_time,
        low_time,
    })
}

fn positive_bd(tool: &str, value: &BigDecimal, name: &str) -> Result<(), String> {
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

/// # Panics
///
/// Panics only if the hard-coded `ln(2)` literal fails to parse — which is
/// impossible for the compile-time constant used here.
#[must_use]
pub fn timer_555_astable(r1: &str, r2: &str, c: &str) -> String {
    let inputs = match parse_timer_astable(r1, r2, c) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let timing = match compute_timer_astable(&inputs) {
        Ok(v) => v,
        Err(e) => return e,
    };
    Response::ok(TIMER_555_ASTABLE)
        .field("FREQUENCY", strip_plain(&timing.freq))
        .field("PERIOD", strip_plain(&timing.period))
        .field("DUTY_CYCLE", strip_plain(&timing.duty))
        .field("HIGH_TIME", strip_plain(&timing.high_time))
        .field("LOW_TIME", strip_plain(&timing.low_time))
        .build()
}

/// 555-timer monostable pulse width: 1.1 * R * C.
///
/// # Panics
///
/// Panics only if the hard-coded `"1.1"` literal fails to parse as a
/// `BigDecimal` — which is impossible for this compile-time constant.
#[must_use]
pub fn timer_555_monostable(r: &str, c: &str) -> String {
    let r_v = match parse_bd(TIMER_555_MONOSTABLE, r, "r") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let c_v = match parse_bd(TIMER_555_MONOSTABLE, c, "c") {
        Ok(v) => v,
        Err(e) => return e,
    };
    if r_v.is_zero() || r_v.is_negative() {
        return error_with_detail(
            TIMER_555_MONOSTABLE,
            ErrorCode::InvalidInput,
            "r must be positive",
            &format!("r={r}"),
        );
    }
    if c_v.is_zero() || c_v.is_negative() {
        return error_with_detail(
            TIMER_555_MONOSTABLE,
            ErrorCode::InvalidInput,
            "c must be positive",
            &format!("c={c}"),
        );
    }
    let constant: BigDecimal = "1.1".parse().expect("valid constant");
    let pulse = mul_ctx(&mul_ctx(&constant, &r_v), &c_v);
    Response::ok(TIMER_555_MONOSTABLE)
        .field("PULSE_WIDTH", strip_plain(&pulse))
        .build()
}

/// Convert between frequency and period (reciprocal).
#[must_use]
pub fn frequency_period(value: &str, mode: &str) -> String {
    match mode {
        "freqToPeriod" | "periodToFreq" => {}
        _ => {
            return error_with_detail(
                FREQUENCY_PERIOD,
                ErrorCode::InvalidInput,
                "mode must be 'freqToPeriod' or 'periodToFreq'",
                &format!("mode={mode}"),
            );
        }
    }
    let val = match parse_bd(FREQUENCY_PERIOD, value, "value") {
        Ok(v) => v,
        Err(e) => return e,
    };
    if val.is_zero() || val.is_negative() {
        return error(
            FREQUENCY_PERIOD,
            ErrorCode::InvalidInput,
            "value must be positive",
        );
    }
    let out = div_scaled(&BigDecimal::from(1), &val);
    Response::ok(FREQUENCY_PERIOD)
        .result(strip_plain(&out))
        .build()
}

/// Nyquist minimum sampling rate: 2 × bandwidth.
#[must_use]
pub fn nyquist_rate(bandwidth_hz: &str) -> String {
    let bw = match parse_bd(NYQUIST_RATE, bandwidth_hz, "bandwidth") {
        Ok(v) => v,
        Err(e) => return e,
    };
    if bw.is_zero() || bw.is_negative() {
        return error_with_detail(
            NYQUIST_RATE,
            ErrorCode::InvalidInput,
            "bandwidth must be positive",
            &format!("bandwidthHz={bandwidth_hz}"),
        );
    }
    let min_rate = mul_ctx(&bw, &BigDecimal::from(2));
    Response::ok(NYQUIST_RATE)
        .result(strip_plain(&min_rate))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_base_decimal_to_hex() {
        assert_eq!(convert_base("255", 10, 16), "CONVERT_BASE: OK | RESULT: FF");
    }

    #[test]
    fn convert_base_binary_to_decimal() {
        assert_eq!(convert_base("1010", 2, 10), "CONVERT_BASE: OK | RESULT: 10");
    }

    #[test]
    fn convert_base_hex_to_binary() {
        assert_eq!(
            convert_base("FF", 16, 2),
            "CONVERT_BASE: OK | RESULT: 11111111"
        );
    }

    #[test]
    fn convert_base_bad_base_errors() {
        assert_eq!(
            convert_base("10", 1, 10),
            "CONVERT_BASE: ERROR\nREASON: [OUT_OF_RANGE] base must be between 2 and 36\nDETAIL: base=1"
        );
        assert_eq!(
            convert_base("10", 10, 37),
            "CONVERT_BASE: ERROR\nREASON: [OUT_OF_RANGE] base must be between 2 and 36\nDETAIL: base=37"
        );
    }

    #[test]
    fn convert_base_invalid_digit_errors() {
        assert_eq!(
            convert_base("XYZ", 10, 16),
            "CONVERT_BASE: ERROR\nREASON: [PARSE_ERROR] invalid number for base 10\nDETAIL: value=XYZ"
        );
    }

    #[test]
    fn twos_complement_negative_eight_bit() {
        assert_eq!(
            twos_complement("-5", 8, "toTwos"),
            "TWOS_COMPLEMENT: OK | RESULT: 11111011"
        );
    }

    #[test]
    fn twos_complement_positive_eight_bit() {
        assert_eq!(
            twos_complement("5", 8, "toTwos"),
            "TWOS_COMPLEMENT: OK | RESULT: 00000101"
        );
    }

    #[test]
    fn twos_complement_roundtrip_negative() {
        assert_eq!(
            twos_complement("11111011", 8, "fromTwos"),
            "TWOS_COMPLEMENT: OK | RESULT: -5"
        );
    }

    #[test]
    fn twos_complement_roundtrip_positive() {
        assert_eq!(
            twos_complement("00000101", 8, "fromTwos"),
            "TWOS_COMPLEMENT: OK | RESULT: 5"
        );
    }

    #[test]
    fn twos_complement_bad_bits_errors() {
        assert_eq!(
            twos_complement("5", 0, "toTwos"),
            "TWOS_COMPLEMENT: ERROR\nREASON: [OUT_OF_RANGE] bit width must be between 1 and 64\nDETAIL: bits=0"
        );
        assert_eq!(
            twos_complement("5", 65, "toTwos"),
            "TWOS_COMPLEMENT: ERROR\nREASON: [OUT_OF_RANGE] bit width must be between 1 and 64\nDETAIL: bits=65"
        );
    }

    #[test]
    fn twos_complement_bad_direction_errors() {
        assert_eq!(
            twos_complement("5", 8, "toward"),
            "TWOS_COMPLEMENT: ERROR\nREASON: [INVALID_INPUT] direction must be 'toTwos' or 'fromTwos'\nDETAIL: direction=toward"
        );
    }

    #[test]
    fn twos_complement_from_rejects_string_longer_than_bit_width() {
        // `111111111` is 9 bits; decoding it against bits=8 used to yield 511
        // (unsigned 9-bit value) even though 511 is outside 8-bit signed
        // range. Now rejected with OUT_OF_RANGE.
        let out = twos_complement("111111111", 8, "fromTwos");
        assert!(out.starts_with("TWOS_COMPLEMENT: ERROR"));
        assert!(out.contains("OUT_OF_RANGE"));
        assert!(out.contains("longer than 8-bit"));
    }

    #[test]
    fn twos_complement_from_accepts_short_string() {
        // Short strings are still valid — they just encode non-negative
        // values with implicit top-zero padding.
        assert_eq!(
            twos_complement("10", 8, "fromTwos"),
            "TWOS_COMPLEMENT: OK | RESULT: 2"
        );
    }

    #[test]
    fn twos_complement_value_above_signed_range_errors() {
        // 1024 does not fit in a signed 8-bit integer (max 127). The original
        // implementation silently masked and returned "00000000".
        assert_eq!(
            twos_complement("1024", 8, "toTwos"),
            "TWOS_COMPLEMENT: ERROR\nREASON: [OUT_OF_RANGE] value is outside the 8-bit signed range\nDETAIL: value=1024, min=-128, max=127"
        );
    }

    #[test]
    fn twos_complement_value_below_signed_range_errors() {
        assert_eq!(
            twos_complement("-129", 8, "toTwos"),
            "TWOS_COMPLEMENT: ERROR\nREASON: [OUT_OF_RANGE] value is outside the 8-bit signed range\nDETAIL: value=-129, min=-128, max=127"
        );
    }

    #[test]
    fn twos_complement_boundary_values_accepted() {
        assert_eq!(
            twos_complement("127", 8, "toTwos"),
            "TWOS_COMPLEMENT: OK | RESULT: 01111111"
        );
        assert_eq!(
            twos_complement("-128", 8, "toTwos"),
            "TWOS_COMPLEMENT: OK | RESULT: 10000000"
        );
    }

    #[test]
    fn gray_code_to_gray() {
        assert_eq!(gray_code("1010", "toGray"), "GRAY_CODE: OK | RESULT: 1111");
    }

    #[test]
    fn gray_code_from_gray_roundtrip() {
        assert_eq!(
            gray_code("1111", "fromGray"),
            "GRAY_CODE: OK | RESULT: 1010"
        );
    }

    #[test]
    fn gray_code_bad_direction_errors() {
        assert_eq!(
            gray_code("1010", "flip"),
            "GRAY_CODE: ERROR\nREASON: [INVALID_INPUT] direction must be 'toGray' or 'fromGray'\nDETAIL: direction=flip"
        );
    }

    #[test]
    fn gray_code_invalid_binary_errors() {
        assert_eq!(
            gray_code("102", "toGray"),
            "GRAY_CODE: ERROR\nREASON: [PARSE_ERROR] value is not a valid binary string\nDETAIL: value=102"
        );
    }

    #[test]
    fn bitwise_and_ones() {
        assert_eq!(bitwise_op("12", "10", "AND"), "BITWISE_OP: OK | RESULT: 8");
    }

    #[test]
    fn bitwise_or_simple() {
        assert_eq!(bitwise_op("12", "10", "OR"), "BITWISE_OP: OK | RESULT: 14");
    }

    #[test]
    fn bitwise_xor_simple() {
        assert_eq!(bitwise_op("12", "10", "XOR"), "BITWISE_OP: OK | RESULT: 6");
    }

    #[test]
    fn bitwise_shl() {
        assert_eq!(bitwise_op("1", "4", "SHL"), "BITWISE_OP: OK | RESULT: 16");
    }

    #[test]
    fn bitwise_shr() {
        assert_eq!(bitwise_op("16", "4", "SHR"), "BITWISE_OP: OK | RESULT: 1");
    }

    #[test]
    fn bitwise_unknown_op_errors() {
        assert_eq!(
            bitwise_op("1", "1", "NAND"),
            "BITWISE_OP: ERROR\nREASON: [INVALID_INPUT] unknown operation\nDETAIL: operation=NAND"
        );
    }

    #[test]
    fn bitwise_accepts_hex_and_binary_prefixes() {
        assert_eq!(
            bitwise_op("0xFF", "0x0F", "XOR"),
            "BITWISE_OP: OK | RESULT: 240"
        );
        assert_eq!(
            bitwise_op("0b1010", "0b0110", "AND"),
            "BITWISE_OP: OK | RESULT: 2"
        );
        assert_eq!(
            bitwise_op("0o17", "0o7", "OR"),
            "BITWISE_OP: OK | RESULT: 15"
        );
        // Case-insensitive prefix; leading + allowed; negative supported.
        assert_eq!(
            bitwise_op("0X10", "0X01", "OR"),
            "BITWISE_OP: OK | RESULT: 17"
        );
        assert_eq!(
            bitwise_op("-0xFF", "0", "OR"),
            "BITWISE_OP: OK | RESULT: -255"
        );
    }

    #[test]
    fn bitwise_rejects_malformed_literal() {
        assert_eq!(
            bitwise_op("0xZZ", "1", "AND"),
            "BITWISE_OP: ERROR\nREASON: [PARSE_ERROR] operand A is not a valid integer\nDETAIL: a=0xZZ"
        );
        assert_eq!(
            bitwise_op("1", "0b", "AND"),
            "BITWISE_OP: ERROR\nREASON: [PARSE_ERROR] operand B is not a valid integer\nDETAIL: b=0b"
        );
    }

    #[test]
    fn adc_resolution_ten_bit() {
        // lsb = 5/1024 to 20 digits HalfUp, step = 1023
        assert_eq!(
            adc_resolution(10, "5"),
            "ADC_RESOLUTION: OK | BITS: 10 | LSB: 0.0048828125 | STEP_COUNT: 1023"
        );
    }

    #[test]
    fn adc_resolution_bad_bits_errors() {
        assert_eq!(
            adc_resolution(0, "5"),
            "ADC_RESOLUTION: ERROR\nREASON: [OUT_OF_RANGE] bit width must be between 1 and 64\nDETAIL: bits=0"
        );
    }

    #[test]
    fn dac_output_midpoint() {
        // bits=10, vref=5, code=512 → 5*512/1024 = 2.5
        assert_eq!(
            dac_output(10, "5", 512),
            "DAC_OUTPUT: OK | BITS: 10 | CODE: 512 | VOUT: 2.5"
        );
    }

    #[test]
    fn dac_output_out_of_range_errors() {
        assert_eq!(
            dac_output(8, "5", 1000),
            "DAC_OUTPUT: ERROR\nREASON: [OUT_OF_RANGE] code must be between 0 and 255\nDETAIL: code=1000"
        );
        assert_eq!(
            dac_output(8, "5", -1),
            "DAC_OUTPUT: ERROR\nREASON: [OUT_OF_RANGE] code must be between 0 and 255\nDETAIL: code=-1"
        );
    }

    #[test]
    fn timer_555_astable_all_fields_present() {
        let out = timer_555_astable("1000", "1000", "0.000001");
        assert!(
            out.starts_with("TIMER_555_ASTABLE: OK | FREQUENCY: "),
            "got: {out}"
        );
        assert!(out.contains(" | PERIOD: "));
        assert!(out.contains(" | DUTY_CYCLE: "));
        assert!(out.contains(" | HIGH_TIME: "));
        assert!(out.contains(" | LOW_TIME: "));
        // Duty cycle ≈ 66.666...
        let anchor = out.find(" | DUTY_CYCLE: ").unwrap() + " | DUTY_CYCLE: ".len();
        let rest = &out[anchor..];
        let end = rest.find(' ').map_or(rest.len(), |i| i);
        let duty: f64 = rest[..end].parse().unwrap();
        assert!((duty - 66.6666).abs() < 0.01, "duty was {duty}");
    }

    #[test]
    fn timer_555_monostable_formula() {
        assert_eq!(
            timer_555_monostable("1000", "0.000001"),
            "TIMER_555_MONOSTABLE: OK | PULSE_WIDTH: 0.0011"
        );
    }

    #[test]
    fn timer_555_astable_rejects_zero_r1() {
        assert_eq!(
            timer_555_astable("0", "1000", "0.000001"),
            "TIMER_555_ASTABLE: ERROR\nREASON: [INVALID_INPUT] r1 must be positive\nDETAIL: r1=0"
        );
    }

    #[test]
    fn timer_555_astable_rejects_negative_r2() {
        assert_eq!(
            timer_555_astable("1000", "-5", "0.000001"),
            "TIMER_555_ASTABLE: ERROR\nREASON: [INVALID_INPUT] r2 must be positive\nDETAIL: r2=-5"
        );
    }

    #[test]
    fn timer_555_monostable_rejects_negative_c() {
        assert_eq!(
            timer_555_monostable("1000", "-0.000001"),
            "TIMER_555_MONOSTABLE: ERROR\nREASON: [INVALID_INPUT] c must be positive\nDETAIL: c=-0.000001"
        );
    }

    #[test]
    fn timer_555_monostable_rejects_zero_r() {
        assert_eq!(
            timer_555_monostable("0", "0.000001"),
            "TIMER_555_MONOSTABLE: ERROR\nREASON: [INVALID_INPUT] r must be positive\nDETAIL: r=0"
        );
    }

    #[test]
    fn frequency_period_freq_to_period() {
        assert_eq!(
            frequency_period("1000", "freqToPeriod"),
            "FREQUENCY_PERIOD: OK | RESULT: 0.001"
        );
    }

    #[test]
    fn frequency_period_period_to_freq() {
        assert_eq!(
            frequency_period("0.001", "periodToFreq"),
            "FREQUENCY_PERIOD: OK | RESULT: 1000"
        );
    }

    #[test]
    fn frequency_period_bad_mode_errors() {
        assert_eq!(
            frequency_period("10", "flip"),
            "FREQUENCY_PERIOD: ERROR\nREASON: [INVALID_INPUT] mode must be 'freqToPeriod' or 'periodToFreq'\nDETAIL: mode=flip"
        );
    }

    #[test]
    fn frequency_period_non_positive_errors() {
        assert_eq!(
            frequency_period("0", "freqToPeriod"),
            "FREQUENCY_PERIOD: ERROR\nREASON: [INVALID_INPUT] value must be positive"
        );
        assert_eq!(
            frequency_period("-5", "periodToFreq"),
            "FREQUENCY_PERIOD: ERROR\nREASON: [INVALID_INPUT] value must be positive"
        );
    }

    #[test]
    fn nyquist_rate_audio() {
        assert_eq!(nyquist_rate("20000"), "NYQUIST_RATE: OK | RESULT: 40000");
    }

    #[test]
    fn nyquist_rate_rejects_zero() {
        // Regression: previously returned 0 for bandwidth=0 instead of erroring.
        assert_eq!(
            nyquist_rate("0"),
            "NYQUIST_RATE: ERROR\nREASON: [INVALID_INPUT] bandwidth must be positive\nDETAIL: bandwidthHz=0"
        );
    }

    #[test]
    fn nyquist_rate_rejects_negative() {
        // Regression: previously returned -200 for bandwidth=-100.
        assert_eq!(
            nyquist_rate("-100"),
            "NYQUIST_RATE: ERROR\nREASON: [INVALID_INPUT] bandwidth must be positive\nDETAIL: bandwidthHz=-100"
        );
    }
}
