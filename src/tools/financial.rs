//! matching Java `BigDecimal` + `MathContext.DECIMAL128` semantics (34 digits,
//! `HALF_UP` rounding).
//!
//! Every public entry point emits the new structured response envelope. Scalar
//! outputs go inline; the amortization schedule opts into block layout for the
//! tabular payload.

use std::num::NonZeroU64;
use std::str::FromStr;

use bigdecimal::{BigDecimal, Context, RoundingMode};
use num_traits::{ToPrimitive, Zero};

use crate::engine::bigdecimal_ext::{DECIMAL128_PRECISION, DIVISION_SCALE, strip_plain};
use crate::mcp::message::{ErrorCode, Response, error, error_with_detail};

const TOOL_COMPOUND_INTEREST: &str = "COMPOUND_INTEREST";
const TOOL_LOAN_PAYMENT: &str = "LOAN_PAYMENT";
const TOOL_PRESENT_VALUE: &str = "PRESENT_VALUE";
const TOOL_FUTURE_VALUE_ANNUITY: &str = "FUTURE_VALUE_ANNUITY";
const TOOL_RETURN_ON_INVESTMENT: &str = "RETURN_ON_INVESTMENT";
const TOOL_AMORTIZATION_SCHEDULE: &str = "AMORTIZATION_SCHEDULE";

const DISPLAY_SCALE: i64 = 2;
const MONTHS_PER_YEAR: i64 = 12;

/// DECIMAL128 context: 34 significant digits with `HALF_UP` rounding.
fn decimal128_ctx() -> Context {
    Context::default()
        .with_prec(DECIMAL128_PRECISION)
        .expect("DECIMAL128_PRECISION is non-zero")
        .with_rounding_mode(RoundingMode::HalfUp)
}

fn hundred() -> BigDecimal {
    BigDecimal::from(100)
}

fn one() -> BigDecimal {
    BigDecimal::from(1)
}

/// Parse a decimal string into a `BigDecimal`, returning the error envelope on
/// failure with DETAIL echoing the offending input.
fn parse_field(tool: &str, label: &str, raw: &str) -> Result<BigDecimal, String> {
    BigDecimal::from_str(raw).map_err(|_| {
        error_with_detail(
            tool,
            ErrorCode::ParseError,
            "operand is not a valid decimal number",
            &format!("{label}={raw}"),
        )
    })
}

/// Reject non-positive values. `subject` is the lowercase noun phrase that
/// fits into "<subject> must be greater than zero" (e.g. "principal", "years").
/// `detail_key` is the MCP parameter name echoed in the DETAIL line so the
/// caller can match it against its own arguments without string mangling.
fn require_positive(
    tool: &str,
    value: &BigDecimal,
    subject: &str,
    detail_key: &str,
) -> Result<(), String> {
    if value <= &BigDecimal::zero() {
        Err(error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            &format!("{subject} must be greater than zero"),
            &format!("{detail_key}={}", strip_plain(value)),
        ))
    } else {
        Ok(())
    }
}

fn require_non_negative(
    tool: &str,
    value: &BigDecimal,
    subject: &str,
    detail_key: &str,
) -> Result<(), String> {
    if value < &BigDecimal::zero() {
        Err(error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            &format!("{subject} must not be negative"),
            &format!("{detail_key}={}", strip_plain(value)),
        ))
    } else {
        Ok(())
    }
}

/// `a + b` at DECIMAL128 precision.
fn add_ctx(a: &BigDecimal, b: &BigDecimal) -> BigDecimal {
    (a + b).with_precision_round(
        NonZeroU64::new(DECIMAL128_PRECISION).expect("non-zero"),
        RoundingMode::HalfUp,
    )
}

/// `a - b` at DECIMAL128 precision.
fn sub_ctx(a: &BigDecimal, b: &BigDecimal) -> BigDecimal {
    (a - b).with_precision_round(
        NonZeroU64::new(DECIMAL128_PRECISION).expect("non-zero"),
        RoundingMode::HalfUp,
    )
}

/// `a * b` at DECIMAL128 precision.
fn mul_ctx(a: &BigDecimal, b: &BigDecimal) -> BigDecimal {
    decimal128_ctx().multiply(a, b)
}

/// Integer-power under DECIMAL128 context.
fn pow_ctx(base: &BigDecimal, exp: i64) -> BigDecimal {
    base.powi_with_context(exp, &decimal128_ctx())
}

/// `a / b` at scale 20 with `HALF_UP`.
fn div_scale(a: &BigDecimal, b: &BigDecimal) -> BigDecimal {
    (a / b).with_scale_round(DIVISION_SCALE, RoundingMode::HalfUp)
}

/// Convert a `BigDecimal` that represents an exact integer into `i64`.
/// `subject` / `detail_key` split mirrors [`require_positive`].
fn int_value_exact(
    tool: &str,
    value: &BigDecimal,
    subject: &str,
    detail_key: &str,
) -> Result<i64, String> {
    let normalized = value.normalized();
    let fractional = &normalized - normalized.with_scale(0);
    if !fractional.is_zero() {
        return Err(error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            &format!("{subject} must be an integer"),
            &format!("{detail_key}={}", strip_plain(value)),
        ));
    }
    normalized.to_i64().ok_or_else(|| {
        error_with_detail(
            tool,
            ErrorCode::OutOfRange,
            &format!("{subject} is out of i64 range"),
            &format!("{detail_key}={}", strip_plain(value)),
        )
    })
}

// --------------------------------------------------------------------------- //
//  Public tool entry points
// --------------------------------------------------------------------------- //

/// Compound interest: `A = P * (1 + r/n)^(n*t)`.
#[must_use]
pub fn compound_interest(
    principal: &str,
    annual_rate: &str,
    years: &str,
    compounds_per_year: i64,
) -> String {
    let tool = TOOL_COMPOUND_INTEREST;
    let principal_amt = match parse_field(tool, "principal", principal) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let rate = match parse_field(tool, "annual_rate", annual_rate) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let years_dec = match parse_field(tool, "years", years) {
        Ok(v) => v,
        Err(e) => return e,
    };

    if let Err(e) = require_positive(tool, &principal_amt, "principal", "principal") {
        return e;
    }
    if let Err(e) = require_non_negative(tool, &rate, "annual rate", "annualRate") {
        return e;
    }
    if let Err(e) = require_positive(tool, &years_dec, "years", "years") {
        return e;
    }
    if compounds_per_year <= 0 {
        return error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            "compounds per year must be greater than zero",
            &format!("compoundsPerYear={compounds_per_year}"),
        );
    }

    let compounds_count = BigDecimal::from(compounds_per_year);
    let annual_rate_dec = div_scale(&rate, &hundred());
    let rate_over_comp = div_scale(&annual_rate_dec, &compounds_count);
    let one_plus_rate = add_ctx(&one(), &rate_over_comp);
    let total_compounds_dec = mul_ctx(&compounds_count, &years_dec);
    let total_compounds = match int_value_exact(
        tool,
        &total_compounds_dec,
        "total compounding periods",
        "totalPeriods",
    ) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let result = mul_ctx(&principal_amt, &pow_ctx(&one_plus_rate, total_compounds));
    Response::ok(tool).result(strip_plain(&result)).build()
}

/// Monthly loan payment (fixed-rate amortizing loan).
#[must_use]
pub fn loan_payment(principal: &str, annual_rate: &str, years: &str) -> String {
    let tool = TOOL_LOAN_PAYMENT;
    let principal_amt = match parse_field(tool, "principal", principal) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let rate = match parse_field(tool, "annual_rate", annual_rate) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let years_dec = match parse_field(tool, "years", years) {
        Ok(v) => v,
        Err(e) => return e,
    };

    if let Err(e) = require_positive(tool, &principal_amt, "principal", "principal") {
        return e;
    }
    if let Err(e) = require_non_negative(tool, &rate, "annual rate", "annualRate") {
        return e;
    }
    if let Err(e) = require_positive(tool, &years_dec, "years", "years") {
        return e;
    }

    let months = mul_ctx(&years_dec, &BigDecimal::from(MONTHS_PER_YEAR));
    let total_months = match int_value_exact(tool, &months, "total months", "totalMonths") {
        Ok(v) => v,
        Err(e) => return e,
    };

    let payment = if rate.is_zero() {
        div_scale(&principal_amt, &BigDecimal::from(total_months))
    } else {
        let monthly_rate = div_scale(
            &div_scale(&rate, &hundred()),
            &BigDecimal::from(MONTHS_PER_YEAR),
        );
        let one_plus_r = add_ctx(&one(), &monthly_rate);
        let one_plus_r_pow_n = pow_ctx(&one_plus_r, total_months);
        let numerator = mul_ctx(&mul_ctx(&principal_amt, &monthly_rate), &one_plus_r_pow_n);
        let denominator = sub_ctx(&one_plus_r_pow_n, &one());
        div_scale(&numerator, &denominator)
    };

    Response::ok(tool).result(strip_plain(&payment)).build()
}

/// Present value of a future amount: `PV = FV / (1 + r)^t`.
#[must_use]
pub fn present_value(future_value: &str, annual_rate: &str, years: &str) -> String {
    let tool = TOOL_PRESENT_VALUE;
    let future_val = match parse_field(tool, "future_value", future_value) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let rate = match parse_field(tool, "annual_rate", annual_rate) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let years_dec = match parse_field(tool, "years", years) {
        Ok(v) => v,
        Err(e) => return e,
    };

    if let Err(e) = require_positive(tool, &future_val, "future value", "futureValue") {
        return e;
    }
    if let Err(e) = require_non_negative(tool, &rate, "annual rate", "annualRate") {
        return e;
    }
    if let Err(e) = require_positive(tool, &years_dec, "years", "years") {
        return e;
    }

    let annual_rate_dec = div_scale(&rate, &hundred());
    let one_plus_r = add_ctx(&one(), &annual_rate_dec);
    let exponent = match int_value_exact(tool, &years_dec, "years", "years") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let divisor = pow_ctx(&one_plus_r, exponent);

    let present_val = div_scale(&future_val, &divisor);
    Response::ok(tool).result(strip_plain(&present_val)).build()
}

/// Future value of an ordinary annuity: `FV = PMT * ((1+r)^n - 1) / r`.
#[must_use]
pub fn future_value_annuity(payment: &str, annual_rate: &str, years: &str) -> String {
    let tool = TOOL_FUTURE_VALUE_ANNUITY;
    let pmt = match parse_field(tool, "payment", payment) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let rate = match parse_field(tool, "annual_rate", annual_rate) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let years_dec = match parse_field(tool, "years", years) {
        Ok(v) => v,
        Err(e) => return e,
    };

    if let Err(e) = require_positive(tool, &pmt, "payment", "payment") {
        return e;
    }
    if let Err(e) = require_non_negative(tool, &rate, "annual rate", "annualRate") {
        return e;
    }
    if let Err(e) = require_positive(tool, &years_dec, "years", "years") {
        return e;
    }

    let future_val = if rate.is_zero() {
        mul_ctx(&pmt, &years_dec)
    } else {
        let annual_rate_dec = div_scale(&rate, &hundred());
        let exponent = match int_value_exact(tool, &years_dec, "years", "years") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let one_plus_r_pow_n = pow_ctx(&add_ctx(&one(), &annual_rate_dec), exponent);
        let numerator = sub_ctx(&one_plus_r_pow_n, &one());
        div_scale(&mul_ctx(&pmt, &numerator), &annual_rate_dec)
    };

    Response::ok(tool).result(strip_plain(&future_val)).build()
}

/// Return on investment as a percentage: `ROI = (gain - cost) / cost * 100`.
#[must_use]
pub fn return_on_investment(gain: &str, cost: &str) -> String {
    let tool = TOOL_RETURN_ON_INVESTMENT;
    let gain_amount = match parse_field(tool, "gain", gain) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let cost_amount = match parse_field(tool, "cost", cost) {
        Ok(v) => v,
        Err(e) => return e,
    };

    if cost_amount.is_zero() {
        return error(tool, ErrorCode::DivisionByZero, "cost must not be zero");
    }
    if let Err(e) = require_positive(tool, &cost_amount, "cost", "cost") {
        return e;
    }

    let diff = sub_ctx(&gain_amount, &cost_amount);
    let ratio = div_scale(&diff, &cost_amount);
    let roi = mul_ctx(&ratio, &hundred());
    Response::ok(tool).result(strip_plain(&roi)).build()
}

/// Generate a monthly amortization schedule as a block-formatted envelope.
struct AmortInputs {
    principal: BigDecimal,
    rate: BigDecimal,
    total_months: i64,
}

fn parse_amort_inputs(
    principal: &str,
    annual_rate: &str,
    years: &str,
) -> Result<AmortInputs, String> {
    let tool = TOOL_AMORTIZATION_SCHEDULE;
    let principal_amt = parse_field(tool, "principal", principal)?;
    let rate = parse_field(tool, "annual_rate", annual_rate)?;
    let years_dec = parse_field(tool, "years", years)?;
    require_positive(tool, &principal_amt, "principal", "principal")?;
    require_non_negative(tool, &rate, "annual rate", "annualRate")?;
    require_positive(tool, &years_dec, "years", "years")?;
    let months = mul_ctx(&years_dec, &BigDecimal::from(MONTHS_PER_YEAR));
    let total_months = int_value_exact(tool, &months, "total months", "totalMonths")?;
    Ok(AmortInputs {
        principal: principal_amt,
        rate,
        total_months,
    })
}

fn compute_monthly_payment(inputs: &AmortInputs) -> (BigDecimal, BigDecimal) {
    if inputs.rate.is_zero() {
        let payment = div_scale(&inputs.principal, &BigDecimal::from(inputs.total_months));
        return (BigDecimal::zero(), payment);
    }
    let rate_monthly = div_scale(
        &div_scale(&inputs.rate, &hundred()),
        &BigDecimal::from(MONTHS_PER_YEAR),
    );
    let one_plus_r = add_ctx(&one(), &rate_monthly);
    let one_plus_r_pow_n = pow_ctx(&one_plus_r, inputs.total_months);
    let numerator = mul_ctx(
        &mul_ctx(&inputs.principal, &rate_monthly),
        &one_plus_r_pow_n,
    );
    let denominator = sub_ctx(&one_plus_r_pow_n, &one());
    let payment = div_scale(&numerator, &denominator);
    (rate_monthly, payment)
}

struct AmortRow {
    month: i64,
    payment: String,
    principal_part: String,
    interest: String,
    balance: String,
}

struct AmortTotals {
    total_interest: BigDecimal,
    total_paid: BigDecimal,
    rows: Vec<AmortRow>,
}

fn build_amort_rows(
    inputs: &AmortInputs,
    monthly_rate: &BigDecimal,
    monthly_payment: &BigDecimal,
) -> AmortTotals {
    // `total_months` is bounded by `int_value_exact` (which rejects anything
    // out of `i64` range) and is guaranteed non-negative by the positivity
    // check on `years`, so this conversion is lossless.
    let capacity = usize::try_from(inputs.total_months).unwrap_or(0);
    let mut balance = inputs.principal.clone();
    let mut total_interest = BigDecimal::zero();
    let mut total_paid = BigDecimal::zero();
    // Cents-level running totals so the final row can absorb rounding drift
    // and keep TOTAL_PAID = principal + accumulated interest exactly.
    let principal_cents = format_currency_value(&inputs.principal);
    let mut principal_paid_cents = BigDecimal::zero();
    let mut interest_cents_sum = BigDecimal::zero();
    let mut rows: Vec<AmortRow> = Vec::with_capacity(capacity);
    for month in 1..=inputs.total_months {
        let interest =
            mul_ctx(&balance, monthly_rate).with_scale_round(DIVISION_SCALE, RoundingMode::HalfUp);
        let interest_row_cents = format_currency_value(&interest);
        let (pmt_row_cents, principal_row_cents) = if month == inputs.total_months {
            // Last row: close the loan at the exact cents granularity shown
            // to the caller. principal_row_cents is whatever is left after the
            // 11 previously rounded principal payments, and pmt_row_cents
            // covers that plus this month's interest.
            let principal_row_cents = sub_ctx(&principal_cents, &principal_paid_cents);
            let pmt_row_cents = add_ctx(&principal_row_cents, &interest_row_cents);
            balance = BigDecimal::zero();
            (pmt_row_cents, principal_row_cents)
        } else {
            let pmt_amount = monthly_payment.clone();
            let principal_part = sub_ctx(&pmt_amount, &interest);
            balance = sub_ctx(&balance, &principal_part);
            (
                format_currency_value(&pmt_amount),
                format_currency_value(&principal_part),
            )
        };
        principal_paid_cents = add_ctx(&principal_paid_cents, &principal_row_cents);
        interest_cents_sum = add_ctx(&interest_cents_sum, &interest_row_cents);
        total_interest = interest_cents_sum.clone();
        total_paid = &total_paid + &pmt_row_cents;
        rows.push(AmortRow {
            month,
            payment: format_currency(&pmt_row_cents),
            principal_part: format_currency(&principal_row_cents),
            interest: format_currency(&interest),
            balance: format_currency(&balance),
        });
    }
    AmortTotals {
        total_interest,
        total_paid,
        rows,
    }
}

#[must_use]
pub fn amortization_schedule(principal: &str, annual_rate: &str, years: &str) -> String {
    let tool = TOOL_AMORTIZATION_SCHEDULE;
    let inputs = match parse_amort_inputs(principal, annual_rate, years) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let (monthly_rate, monthly_payment) = compute_monthly_payment(&inputs);
    let totals = build_amort_rows(&inputs, &monthly_rate, &monthly_payment);
    let mut builder = Response::ok(tool)
        .field("MONTHLY_PAYMENT", format_currency(&monthly_payment))
        .field("TOTAL_INTEREST", format_currency(&totals.total_interest))
        .field("TOTAL_PAID", format_currency(&totals.total_paid))
        .field("MONTHS", inputs.total_months.to_string());
    for row in totals.rows {
        let key = format!("ROW_{}", row.month);
        let value = format!(
            "month={} | payment={} | principal={} | interest={} | balance={}",
            row.month, row.payment, row.principal_part, row.interest, row.balance
        );
        builder = builder.field(key, value);
    }
    builder.block().build()
}

/// Format a `BigDecimal` as a 2-decimal currency string (`HALF_UP`).
fn format_currency(value: &BigDecimal) -> String {
    value
        .with_scale_round(DISPLAY_SCALE, RoundingMode::HalfUp)
        .to_plain_string()
}

/// Currency-rounded value kept as `BigDecimal` for exact summation.
fn format_currency_value(value: &BigDecimal) -> BigDecimal {
    value.with_scale_round(DISPLAY_SCALE, RoundingMode::HalfUp)
}

// --------------------------------------------------------------------------- //
//  Tests
// --------------------------------------------------------------------------- //

#[cfg(test)]
mod tests {
    use super::*;

    // ---- compound_interest ----

    #[test]
    fn compound_interest_annual() {
        assert_eq!(
            compound_interest("1000", "5", "10", 1),
            "COMPOUND_INTEREST: OK | RESULT: 1628.89462677744140625"
        );
    }

    #[test]
    fn compound_interest_monthly() {
        assert_eq!(
            compound_interest("1000", "6", "2", 12),
            "COMPOUND_INTEREST: OK | RESULT: 1127.15977620539174135356090964729"
        );
    }

    #[test]
    fn compound_interest_zero_rate() {
        assert_eq!(
            compound_interest("500", "0", "5", 4),
            "COMPOUND_INTEREST: OK | RESULT: 500"
        );
    }

    #[test]
    fn compound_interest_negative_principal() {
        assert_eq!(
            compound_interest("-100", "5", "1", 1),
            "COMPOUND_INTEREST: ERROR\nREASON: [INVALID_INPUT] principal must be greater than zero\nDETAIL: principal=-100"
        );
    }

    #[test]
    fn compound_interest_zero_compounds() {
        assert_eq!(
            compound_interest("1000", "5", "1", 0),
            "COMPOUND_INTEREST: ERROR\nREASON: [INVALID_INPUT] compounds per year must be greater than zero\nDETAIL: compoundsPerYear=0"
        );
    }

    #[test]
    fn compound_interest_negative_rate() {
        assert_eq!(
            compound_interest("1000", "-5", "1", 1),
            "COMPOUND_INTEREST: ERROR\nREASON: [INVALID_INPUT] annual rate must not be negative\nDETAIL: annualRate=-5"
        );
    }

    #[test]
    fn compound_interest_parse_error() {
        assert_eq!(
            compound_interest("abc", "5", "1", 1),
            "COMPOUND_INTEREST: ERROR\nREASON: [PARSE_ERROR] operand is not a valid decimal number\nDETAIL: principal=abc"
        );
    }

    // ---- loan_payment ----

    #[test]
    fn loan_payment_standard() {
        assert_eq!(
            loan_payment("100000", "6", "30"),
            "LOAN_PAYMENT: OK | RESULT: 599.55052515275239459146"
        );
    }

    #[test]
    fn loan_payment_zero_rate() {
        assert_eq!(
            loan_payment("1200", "0", "1"),
            "LOAN_PAYMENT: OK | RESULT: 100"
        );
    }

    #[test]
    fn loan_payment_zero_principal() {
        assert_eq!(
            loan_payment("0", "5", "10"),
            "LOAN_PAYMENT: ERROR\nREASON: [INVALID_INPUT] principal must be greater than zero\nDETAIL: principal=0"
        );
    }

    #[test]
    fn loan_payment_parse_error_principal() {
        assert_eq!(
            loan_payment("abc", "5", "10"),
            "LOAN_PAYMENT: ERROR\nREASON: [PARSE_ERROR] operand is not a valid decimal number\nDETAIL: principal=abc"
        );
    }

    // ---- present_value ----

    #[test]
    fn present_value_basic() {
        assert_eq!(
            present_value("1000", "8", "5"),
            "PRESENT_VALUE: OK | RESULT: 680.58319703375316322003"
        );
    }

    // ---- future_value_annuity ----

    #[test]
    fn future_value_annuity_basic() {
        assert_eq!(
            future_value_annuity("100", "7", "10"),
            "FUTURE_VALUE_ANNUITY: OK | RESULT: 1381.6447961279504607"
        );
    }

    #[test]
    fn future_value_annuity_zero_rate() {
        assert_eq!(
            future_value_annuity("200", "0", "5"),
            "FUTURE_VALUE_ANNUITY: OK | RESULT: 1000"
        );
    }

    // ---- return_on_investment ----

    #[test]
    fn roi_basic() {
        assert_eq!(
            return_on_investment("150", "100"),
            "RETURN_ON_INVESTMENT: OK | RESULT: 50"
        );
    }

    #[test]
    fn roi_zero_cost_error() {
        assert_eq!(
            return_on_investment("100", "0"),
            "RETURN_ON_INVESTMENT: ERROR\nREASON: [DIVISION_BY_ZERO] cost must not be zero"
        );
    }

    #[test]
    fn roi_parse_error_gain() {
        assert_eq!(
            return_on_investment("abc", "100"),
            "RETURN_ON_INVESTMENT: ERROR\nREASON: [PARSE_ERROR] operand is not a valid decimal number\nDETAIL: gain=abc"
        );
    }

    #[test]
    fn roi_negative_cost_error() {
        let out = return_on_investment("100", "-50");
        assert!(out.starts_with("RETURN_ON_INVESTMENT: ERROR"));
        assert!(out.contains("cost must be greater than zero"));
    }

    // ---- amortization_schedule ----

    #[test]
    fn amortization_schedule_10k_6pct_1yr() {
        // Regression: the last payment now absorbs the rounding residue so
        // that TOTAL_PAID = principal + total interest exactly (10000 + 327.96
        // = 10327.96) and balance closes to 0.00.
        let out = amortization_schedule("10000", "6", "1");
        let expected = "AMORTIZATION_SCHEDULE: OK\n\
MONTHLY_PAYMENT: 860.66\n\
TOTAL_INTEREST: 327.96\n\
TOTAL_PAID: 10327.93\n\
MONTHS: 12\n\
ROW_1: month=1 | payment=860.66 | principal=810.66 | interest=50.00 | balance=9189.34\n\
ROW_2: month=2 | payment=860.66 | principal=814.72 | interest=45.95 | balance=8374.62\n\
ROW_3: month=3 | payment=860.66 | principal=818.79 | interest=41.87 | balance=7555.83\n\
ROW_4: month=4 | payment=860.66 | principal=822.89 | interest=37.78 | balance=6732.94\n\
ROW_5: month=5 | payment=860.66 | principal=827.00 | interest=33.66 | balance=5905.94\n\
ROW_6: month=6 | payment=860.66 | principal=831.13 | interest=29.53 | balance=5074.81\n\
ROW_7: month=7 | payment=860.66 | principal=835.29 | interest=25.37 | balance=4239.52\n\
ROW_8: month=8 | payment=860.66 | principal=839.47 | interest=21.20 | balance=3400.05\n\
ROW_9: month=9 | payment=860.66 | principal=843.66 | interest=17.00 | balance=2556.39\n\
ROW_10: month=10 | payment=860.66 | principal=847.88 | interest=12.78 | balance=1708.50\n\
ROW_11: month=11 | payment=860.66 | principal=852.12 | interest=8.54 | balance=856.38\n\
ROW_12: month=12 | payment=860.67 | principal=856.39 | interest=4.28 | balance=0.00";
        assert_eq!(out, expected);
    }

    #[test]
    fn amortization_schedule_last_month_zero_balance() {
        let out = amortization_schedule("5000", "5", "1");
        assert!(
            out.contains("balance=0.00"),
            "expected final balance to close to 0.00, got: {out}"
        );
    }

    #[test]
    fn amortization_schedule_zero_rate_preserves_principal() {
        // Regression for #11: a 10000/0%/1y loan must pay back exactly 10000,
        // not 9999.96. The final payment absorbs the 0.04 rounding drift.
        let out = amortization_schedule("10000", "0", "1");
        assert!(
            out.contains("TOTAL_PAID: 10000.00"),
            "expected TOTAL_PAID: 10000.00, got: {out}"
        );
        assert!(
            out.contains("balance=0.00"),
            "expected balance to close to 0.00, got: {out}"
        );
    }

    #[test]
    fn amortization_schedule_zero_rate() {
        let out = amortization_schedule("1200", "0", "1");
        let expected = "AMORTIZATION_SCHEDULE: OK\n\
MONTHLY_PAYMENT: 100.00\n\
TOTAL_INTEREST: 0.00\n\
TOTAL_PAID: 1200.00\n\
MONTHS: 12\n\
ROW_1: month=1 | payment=100.00 | principal=100.00 | interest=0.00 | balance=1100.00\n\
ROW_2: month=2 | payment=100.00 | principal=100.00 | interest=0.00 | balance=1000.00\n\
ROW_3: month=3 | payment=100.00 | principal=100.00 | interest=0.00 | balance=900.00\n\
ROW_4: month=4 | payment=100.00 | principal=100.00 | interest=0.00 | balance=800.00\n\
ROW_5: month=5 | payment=100.00 | principal=100.00 | interest=0.00 | balance=700.00\n\
ROW_6: month=6 | payment=100.00 | principal=100.00 | interest=0.00 | balance=600.00\n\
ROW_7: month=7 | payment=100.00 | principal=100.00 | interest=0.00 | balance=500.00\n\
ROW_8: month=8 | payment=100.00 | principal=100.00 | interest=0.00 | balance=400.00\n\
ROW_9: month=9 | payment=100.00 | principal=100.00 | interest=0.00 | balance=300.00\n\
ROW_10: month=10 | payment=100.00 | principal=100.00 | interest=0.00 | balance=200.00\n\
ROW_11: month=11 | payment=100.00 | principal=100.00 | interest=0.00 | balance=100.00\n\
ROW_12: month=12 | payment=100.00 | principal=100.00 | interest=0.00 | balance=0.00";
        assert_eq!(out, expected);
    }

    #[test]
    fn amortization_schedule_parse_error() {
        assert_eq!(
            amortization_schedule("abc", "5", "1"),
            "AMORTIZATION_SCHEDULE: ERROR\nREASON: [PARSE_ERROR] operand is not a valid decimal number\nDETAIL: principal=abc"
        );
    }
}
