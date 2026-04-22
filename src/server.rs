//! MCP server: exposes every tool module under `crate::tools` via a single
//! `#[tool_router]` impl block.
//!
//! Every MCP tool method returns `String`; failures are surfaced inline as
//! `"Error: {message}"` so MCP clients always receive a plain text response
//! instead of a protocol-level error.

use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
};

use crate::tools::{
    analog_electronics, basic, calculus, cooking, datetime, digital_electronics, financial,
    graphing, measure_reference, network, printing, programmable, scientific, unit_converter,
    vector,
};

#[derive(Clone)]
pub struct MathCalcServer {
    #[allow(dead_code)] // consumed by #[tool_handler]
    tool_router: ToolRouter<Self>,
}

impl Default for MathCalcServer {
    fn default() -> Self {
        Self::new()
    }
}

// --------------------------------------------------------------------------- //
//  Parameter structs — deduplicated by shape. `#[serde(rename_all = "camelCase")]`
//  is applied to structs with multi-word fields so the JSON keys match the
//  Java Spring AI convention (e.g. `fromUnit`, `annualRate`).
// --------------------------------------------------------------------------- //

// ---- basic / abs (decimal strings) ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct BinaryDecimalParams {
    /// First operand (decimal string, arbitrary precision).
    pub first: String,
    /// Second operand (decimal string, arbitrary precision).
    pub second: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PowerParams {
    /// Base value (decimal string, arbitrary precision).
    pub base: String,
    /// Non-negative integer exponent.
    pub exponent: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UnaryDecimalParams {
    /// Single decimal operand (arbitrary precision).
    pub value: String,
}

// ---- scientific ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UnaryDoubleParams {
    /// Single numeric operand (double precision).
    pub number: f64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AngleParams {
    /// Angle in degrees (double precision).
    pub degrees: f64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FactorialParams {
    /// Integer in the closed range [0, 20].
    pub num: i64,
}

// ---- programmable ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EvaluateParams {
    /// Arithmetic expression, e.g. `sin(45)+2^3`.
    pub expression: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EvaluateWithVariablesParams {
    /// Arithmetic expression referencing variables, e.g. `2*x + y`.
    pub expression: String,
    /// JSON object mapping variable names to numeric values, e.g. `{"x":3.0,"y":1.0}`.
    pub variables: String,
}

// ---- vector ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct NumbersArrayParams {
    /// Comma-separated list of doubles, e.g. `"1,2,3,4.5"`.
    pub numbers: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct TwoNumberArraysParams {
    /// First CSV array of doubles.
    pub first: String,
    /// Second CSV array of doubles (same length as `first`).
    pub second: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ScaleArrayParams {
    /// CSV array of doubles.
    pub numbers: String,
    /// Scalar multiplier (decimal string).
    pub scalar: String,
}

// ---- financial ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompoundInterestParams {
    /// Initial principal amount (decimal).
    pub principal: String,
    /// Annual interest rate as a percentage (e.g. `5` = 5%).
    pub annual_rate: String,
    /// Number of years (positive decimal).
    pub years: String,
    /// Compounding periods per year (positive integer).
    pub compounds_per_year: i64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoanPaymentParams {
    /// Loan principal amount.
    pub principal: String,
    /// Annual interest rate as a percentage.
    pub annual_rate: String,
    /// Loan term in years.
    pub years: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PresentValueParams {
    /// Future value amount.
    pub future_value: String,
    /// Annual interest rate as a percentage.
    pub annual_rate: String,
    /// Number of years.
    pub years: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FutureValueAnnuityParams {
    /// Periodic payment amount.
    pub payment: String,
    /// Annual interest rate as a percentage.
    pub annual_rate: String,
    /// Number of years.
    pub years: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RoiParams {
    /// Gain from the investment (decimal).
    pub gain: String,
    /// Cost of the investment (decimal, non-zero).
    pub cost: String,
}

// ---- calculus ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DerivativeParams {
    /// Expression in one variable, e.g. `x^2 + sin(x)`.
    pub expression: String,
    /// Variable name used in the expression.
    pub variable: String,
    /// Point at which to evaluate the derivative.
    pub point: f64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct NthDerivativeParams {
    /// Expression in one variable.
    pub expression: String,
    /// Variable name used in the expression.
    pub variable: String,
    /// Point at which to evaluate the derivative.
    pub point: f64,
    /// Derivative order (integer in `[1, 10]`).
    pub order: i32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DefiniteIntegralParams {
    /// Expression in one variable.
    pub expression: String,
    /// Variable name used in the expression.
    pub variable: String,
    /// Lower bound of integration.
    pub lower: f64,
    /// Upper bound of integration.
    pub upper: f64,
}

// ---- unit converter ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConvertParams {
    /// Value to convert (decimal string).
    pub value: String,
    /// Source unit code (e.g. `km`).
    pub from_unit: String,
    /// Target unit code (e.g. `mi`).
    pub to_unit: String,
    /// Category name (e.g. `LENGTH`, case-insensitive).
    pub category: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConvertAutoParams {
    /// Value to convert (decimal string).
    pub value: String,
    /// Source unit code.
    pub from_unit: String,
    /// Target unit code (must share a category with `fromUnit`).
    pub to_unit: String,
}

// ---- cooking (same 3-field shape as ConvertAutoParams but kept separate
//      for documentation / category semantics) ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CookingConvertParams {
    /// Numeric value to convert (decimal string).
    pub value: String,
    /// Source unit code (e.g. `cup`, `tbsp`, `kg`, `c`, `gasmark`).
    pub from_unit: String,
    /// Target unit code.
    pub to_unit: String,
}

// ---- measure reference ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CategoryParams {
    /// Category name (e.g. `LENGTH`, case-insensitive).
    pub category: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FromToUnitParams {
    /// Source unit code.
    pub from_unit: String,
    /// Target unit code.
    pub to_unit: String,
}

// ---- datetime ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConvertTimezoneParams {
    /// Datetime string (ISO-8601, epoch, or common locale pattern).
    pub datetime: String,
    /// Source IANA timezone ID (e.g. `UTC`, `America/Sao_Paulo`).
    pub from_timezone: String,
    /// Target IANA timezone ID.
    pub to_timezone: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FormatDateTimeParams {
    /// Datetime string to reformat.
    pub datetime: String,
    /// Input format keyword (`iso`/`iso-offset`/`iso-local`/`epoch`/`epochmillis`/`rfc1123`) or strptime pattern.
    pub input_format: String,
    /// Output format keyword or strftime pattern.
    pub output_format: String,
    /// IANA timezone ID applied when no zone is present in input.
    pub timezone: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CurrentDateTimeParams {
    /// IANA timezone ID.
    pub timezone: String,
    /// Format keyword or strftime pattern.
    pub format: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListTimezonesParams {
    /// Region prefix (e.g. `Europe`). Empty string or `all` returns every zone.
    pub region: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DateTimeDifferenceParams {
    /// First datetime string.
    pub datetime1: String,
    /// Second datetime string.
    pub datetime2: String,
    /// IANA timezone ID applied when parsing either datetime.
    pub timezone: String,
}

// ---- printing ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct TapeParams {
    /// JSON array of `{op, value}` entries, e.g. `[{"op":"+","value":"10"}]`.
    pub operations: String,
}

// ---- graphing ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PlotFunctionParams {
    /// Expression in one variable, e.g. `x^2`.
    pub expression: String,
    /// Variable name used in the expression.
    pub variable: String,
    /// Minimum x value.
    pub min: f64,
    /// Maximum x value.
    pub max: f64,
    /// Number of sample intervals (positive integer); returns `steps + 1` points.
    pub steps: i32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SolveEquationParams {
    /// Expression in one variable whose root is sought.
    pub expression: String,
    /// Variable name used in the expression.
    pub variable: String,
    /// Initial guess for the Newton-Raphson iteration.
    pub initial_guess: f64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FindRootsParams {
    /// Expression in one variable whose roots are sought.
    pub expression: String,
    /// Variable name used in the expression.
    pub variable: String,
    /// Minimum of the search interval.
    pub min: f64,
    /// Maximum of the search interval.
    pub max: f64,
}

// ---- network ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SubnetParams {
    /// IPv4 or IPv6 address (e.g. `192.168.1.0` or `2001:db8::`).
    pub address: String,
    /// CIDR prefix length (0–32 for IPv4, 0–128 for IPv6).
    pub cidr: i32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddressParams {
    /// IPv4 or IPv6 address.
    pub address: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct BinaryParams {
    /// Binary form of an IP (dot-separated octets for IPv4 or colon-separated 16-bit groups for IPv6).
    pub binary: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DecimalToIpParams {
    /// Unsigned decimal integer string.
    pub decimal: String,
    /// IP version (`4` or `6`).
    pub version: i32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct IpInSubnetParams {
    /// IP address under test.
    pub address: String,
    /// Network address.
    pub network: String,
    /// CIDR prefix length.
    pub cidr: i32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VlsmParams {
    /// Base network in CIDR notation, e.g. `192.168.1.0/24`.
    pub network_cidr: String,
    /// JSON array of required host counts per subnet, e.g. `[50,25,10]`.
    pub host_counts: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SummarizeParams {
    /// JSON array of CIDR subnet strings.
    pub subnets: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransferTimeParams {
    /// File size (decimal string).
    pub file_size: String,
    /// Data-storage unit (e.g. `gb`, `mb`, `kb`, `byte`).
    pub file_size_unit: String,
    /// Bandwidth value (decimal string).
    pub bandwidth: String,
    /// Data-rate unit (e.g. `mbps`, `gbps`).
    pub bandwidth_unit: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThroughputParams {
    /// Data size (decimal string).
    pub data_size: String,
    /// Data-storage unit (e.g. `mb`, `gb`).
    pub data_size_unit: String,
    /// Time value (decimal string).
    pub time: String,
    /// Time unit (e.g. `s`, `min`).
    pub time_unit: String,
    /// Data-rate output unit (e.g. `mbps`).
    pub output_unit: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TcpThroughputParams {
    /// Link bandwidth in Mbps.
    pub bandwidth_mbps: String,
    /// Round-trip time in milliseconds.
    pub rtt_ms: String,
    /// TCP window size in kilobytes.
    pub window_size_kb: String,
}

// ---- analog electronics ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct OhmsLawParams {
    /// Voltage in volts; pass empty string to treat as unknown.
    pub voltage: String,
    /// Current in amperes; pass empty string to treat as unknown.
    pub current: String,
    /// Resistance in ohms; pass empty string to treat as unknown.
    pub resistance: String,
    /// Power in watts; pass empty string to treat as unknown.
    pub power: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CombinationParams {
    /// Comma-separated component values (same unit).
    pub values: String,
    /// Combination mode: `series` or `parallel`.
    pub mode: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct VoltageDividerParams {
    /// Input voltage.
    pub vin: String,
    /// First resistor (ohms).
    pub r1: String,
    /// Second resistor (ohms).
    pub r2: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CurrentDividerParams {
    /// Total current into the parallel pair (amperes).
    pub total_current: String,
    /// First resistor (ohms).
    pub r1: String,
    /// Second resistor (ohms).
    pub r2: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RcTimeParams {
    /// Resistance in ohms.
    pub resistance: String,
    /// Capacitance in farads.
    pub capacitance: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RlTimeParams {
    /// Resistance in ohms.
    pub resistance: String,
    /// Inductance in henries.
    pub inductance: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RlcParams {
    /// Resistance in ohms.
    pub r: String,
    /// Inductance in henries.
    pub l: String,
    /// Capacitance in farads.
    pub c: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ImpedanceParams {
    /// Resistance in ohms.
    pub r: String,
    /// Inductance in henries.
    pub l: String,
    /// Capacitance in farads.
    pub c: String,
    /// Frequency in hertz.
    pub frequency: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DecibelParams {
    /// Value to convert.
    pub value: String,
    /// Mode: `powerToDb`, `voltageToDb`, `dbToPower`, or `dbToVoltage`.
    pub mode: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FilterCutoffParams {
    /// Resistance in ohms.
    pub resistance: String,
    /// Reactive component (capacitance in farads for RC).
    pub reactive: String,
    /// Filter type: `lowpass` or `highpass` (case-insensitive).
    pub filter_type: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct LedResistorParams {
    /// Supply voltage.
    pub vs: String,
    /// LED forward voltage.
    pub vf: String,
    /// LED forward current in amperes.
    pub i_f: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WheatstoneParams {
    /// First resistor (ohms).
    pub r1: String,
    /// Second resistor (ohms).
    pub r2: String,
    /// Third resistor (ohms).
    pub r3: String,
}

// ---- digital electronics ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConvertBaseParams {
    /// Number to convert, in `fromBase` representation.
    pub value: String,
    /// Source base (2..=36).
    pub from_base: i32,
    /// Target base (2..=36).
    pub to_base: i32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct TwosComplementParams {
    /// Decimal value (for `toTwos`) or binary string (for `fromTwos`).
    pub value: String,
    /// Bit width (1..=64).
    pub bits: i32,
    /// Direction: `toTwos` or `fromTwos`.
    pub direction: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GrayCodeParams {
    /// Binary value to convert.
    pub value: String,
    /// Direction: `toGray` or `fromGray`.
    pub direction: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct BitwiseParams {
    /// Operand A (decimal integer string).
    pub a: String,
    /// Operand B (decimal integer or shift amount). Ignored for `NOT`.
    pub b: String,
    /// Operation: `AND`, `OR`, `XOR`, `NOT`, `SHL`, or `SHR`.
    pub operation: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AdcParams {
    /// Bit width (1..=64).
    pub bits: i32,
    /// Reference voltage.
    pub vref: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DacParams {
    /// Bit width (1..=64).
    pub bits: i32,
    /// Reference voltage.
    pub vref: String,
    /// Digital code in `[0, 2^bits - 1]`.
    pub code: i64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct Timer555AstableParams {
    /// R1 resistor (ohms).
    pub r1: String,
    /// R2 resistor (ohms).
    pub r2: String,
    /// Timing capacitor (farads).
    pub c: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct Timer555MonostableParams {
    /// R resistor (ohms).
    pub r: String,
    /// Timing capacitor (farads).
    pub c: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FreqPeriodParams {
    /// Value to convert (frequency in Hz or period in seconds).
    pub value: String,
    /// Mode: `freqToPeriod` or `periodToFreq`.
    pub mode: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NyquistParams {
    /// Signal bandwidth in hertz.
    pub bandwidth_hz: String,
}

// --------------------------------------------------------------------------- //
//  Tool router — one method per MCP tool.
// --------------------------------------------------------------------------- //

#[tool_router]
impl MathCalcServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    // ---- Basic ---------------------------------------------------------- //

    #[tool(description = "Add two numbers. Returns exact arbitrary-precision result.")]
    fn add(&self, Parameters(p): Parameters<BinaryDecimalParams>) -> String {
        basic::add(&p.first, &p.second)
    }

    #[tool(description = "Subtract second from first. Returns exact arbitrary-precision result.")]
    fn subtract(&self, Parameters(p): Parameters<BinaryDecimalParams>) -> String {
        basic::subtract(&p.first, &p.second)
    }

    #[tool(description = "Multiply two numbers. Returns exact arbitrary-precision result.")]
    fn multiply(&self, Parameters(p): Parameters<BinaryDecimalParams>) -> String {
        basic::multiply(&p.first, &p.second)
    }

    #[tool(description = "Divide first by second. 20-digit precision, HALF_UP rounding.")]
    fn divide(&self, Parameters(p): Parameters<BinaryDecimalParams>) -> String {
        basic::divide(&p.first, &p.second)
    }

    #[tool(description = "Raise base to a non-negative integer exponent. Exact result.")]
    fn power(&self, Parameters(p): Parameters<PowerParams>) -> String {
        basic::power(&p.base, &p.exponent)
    }

    #[tool(description = "Remainder of first divided by second. Exact result.")]
    fn modulo(&self, Parameters(p): Parameters<BinaryDecimalParams>) -> String {
        basic::modulo(&p.first, &p.second)
    }

    #[tool(description = "Absolute value of a decimal. Exact result.")]
    fn abs(&self, Parameters(p): Parameters<UnaryDecimalParams>) -> String {
        basic::abs(&p.value)
    }

    // ---- Scientific ----------------------------------------------------- //

    #[tool(description = "Square root of a non-negative number.")]
    fn sqrt(&self, Parameters(p): Parameters<UnaryDoubleParams>) -> String {
        scientific::sqrt(p.number)
    }

    #[tool(description = "Natural logarithm (ln) of a positive number.")]
    fn log(&self, Parameters(p): Parameters<UnaryDoubleParams>) -> String {
        scientific::log(p.number)
    }

    #[tool(description = "Base-10 logarithm of a positive number.")]
    fn log10(&self, Parameters(p): Parameters<UnaryDoubleParams>) -> String {
        scientific::log10(p.number)
    }

    #[tool(description = "Factorial (n!) for integers in the range 0..=20.")]
    fn factorial(&self, Parameters(p): Parameters<FactorialParams>) -> String {
        scientific::factorial(p.num)
    }

    #[tool(description = "Sine of angle in degrees. Exact at notable angles (0/30/45/60/90/...).")]
    fn sin(&self, Parameters(p): Parameters<AngleParams>) -> String {
        scientific::sin(p.degrees)
    }

    #[tool(
        description = "Cosine of angle in degrees. Exact at notable angles (0/30/45/60/90/...)."
    )]
    fn cos(&self, Parameters(p): Parameters<AngleParams>) -> String {
        scientific::cos(p.degrees)
    }

    #[tool(
        description = "Tangent of angle in degrees. Exact at notable angles; error at 90°/270°."
    )]
    fn tan(&self, Parameters(p): Parameters<AngleParams>) -> String {
        scientific::tan(p.degrees)
    }

    // ---- Programmable --------------------------------------------------- //

    #[tool(
        description = "Evaluate an arithmetic expression. Supports + - * / ^ %, parens, and functions sin/cos/tan/log/log10/sqrt/abs/ceil/floor."
    )]
    fn evaluate(&self, Parameters(p): Parameters<EvaluateParams>) -> String {
        programmable::evaluate(&p.expression)
    }

    #[tool(
        name = "evaluateWithVariables",
        description = "Evaluate an arithmetic expression with a JSON variable map, e.g. expression='2*x+y', variables='{\"x\":3,\"y\":1}'."
    )]
    fn evaluate_with_variables(
        &self,
        Parameters(p): Parameters<EvaluateWithVariablesParams>,
    ) -> String {
        programmable::evaluate_with_variables(&p.expression, &p.variables)
    }

    #[tool(
        name = "evaluateExact",
        description = "Evaluate an arithmetic expression at 128-bit precision (astro-float). Returns exact decimals (0.1+0.2 = 0.3)."
    )]
    fn evaluate_exact(&self, Parameters(p): Parameters<EvaluateParams>) -> String {
        programmable::evaluate_exact(&p.expression)
    }

    #[tool(
        name = "evaluateExactWithVariables",
        description = "Exact evaluator with JSON variable map. Variable values may be numbers or decimal strings; strings preserve >15-digit precision."
    )]
    fn evaluate_exact_with_variables(
        &self,
        Parameters(p): Parameters<EvaluateWithVariablesParams>,
    ) -> String {
        programmable::evaluate_exact_with_variables(&p.expression, &p.variables)
    }

    // ---- Vector (SIMD) -------------------------------------------------- //

    #[tool(
        name = "sumArray",
        description = "Sum all elements of a comma-separated numeric array (SIMD-accelerated)."
    )]
    fn sum_array(&self, Parameters(p): Parameters<NumbersArrayParams>) -> String {
        vector::sum_array(&p.numbers)
    }

    #[tool(
        name = "dotProduct",
        description = "Dot product of two comma-separated numeric arrays of equal length."
    )]
    fn dot_product(&self, Parameters(p): Parameters<TwoNumberArraysParams>) -> String {
        vector::dot_product(&p.first, &p.second)
    }

    #[tool(
        name = "scaleArray",
        description = "Multiply every element of a CSV numeric array by a scalar; returns CSV."
    )]
    fn scale_array(&self, Parameters(p): Parameters<ScaleArrayParams>) -> String {
        vector::scale_array(&p.numbers, &p.scalar)
    }

    #[tool(
        name = "magnitudeArray",
        description = "Euclidean magnitude (L2 norm) of a CSV numeric vector."
    )]
    fn magnitude_array(&self, Parameters(p): Parameters<NumbersArrayParams>) -> String {
        vector::magnitude_array(&p.numbers)
    }

    // ---- Financial ------------------------------------------------------ //

    #[tool(
        name = "compoundInterest",
        description = "Future value with compound interest: A = P*(1 + r/n)^(n*t). Rate is a percent (e.g. 5 = 5%)."
    )]
    fn compound_interest(&self, Parameters(p): Parameters<CompoundInterestParams>) -> String {
        financial::compound_interest(&p.principal, &p.annual_rate, &p.years, p.compounds_per_year)
    }

    #[tool(
        name = "loanPayment",
        description = "Fixed monthly payment for an amortizing loan given principal, annual rate (%) and years."
    )]
    fn loan_payment(&self, Parameters(p): Parameters<LoanPaymentParams>) -> String {
        financial::loan_payment(&p.principal, &p.annual_rate, &p.years)
    }

    #[tool(
        name = "presentValue",
        description = "Present value of a future amount: PV = FV / (1 + r)^t. Rate is a percent."
    )]
    fn present_value(&self, Parameters(p): Parameters<PresentValueParams>) -> String {
        financial::present_value(&p.future_value, &p.annual_rate, &p.years)
    }

    #[tool(
        name = "futureValueAnnuity",
        description = "Future value of an ordinary annuity: FV = PMT * ((1+r)^n - 1) / r."
    )]
    fn future_value_annuity(&self, Parameters(p): Parameters<FutureValueAnnuityParams>) -> String {
        financial::future_value_annuity(&p.payment, &p.annual_rate, &p.years)
    }

    #[tool(
        name = "returnOnInvestment",
        description = "Return on investment as a percentage: ROI = (gain - cost) / cost * 100."
    )]
    fn return_on_investment(&self, Parameters(p): Parameters<RoiParams>) -> String {
        financial::return_on_investment(&p.gain, &p.cost)
    }

    #[tool(
        name = "amortizationSchedule",
        description = "Monthly amortization schedule as a JSON array of {month, payment, principal, interest, balance}."
    )]
    fn amortization_schedule(&self, Parameters(p): Parameters<LoanPaymentParams>) -> String {
        financial::amortization_schedule(&p.principal, &p.annual_rate, &p.years)
    }

    // ---- Calculus ------------------------------------------------------- //

    #[tool(
        description = "Numerical derivative (five-point central difference) of an expression at a point."
    )]
    fn derivative(&self, Parameters(p): Parameters<DerivativeParams>) -> String {
        calculus::derivative(&p.expression, &p.variable, p.point)
    }

    #[tool(
        name = "nthDerivative",
        description = "Nth-order numerical derivative of an expression at a point. Order must be in [1, 10]."
    )]
    fn nth_derivative(&self, Parameters(p): Parameters<NthDerivativeParams>) -> String {
        calculus::nth_derivative(&p.expression, &p.variable, p.point, p.order)
    }

    #[tool(
        name = "definiteIntegral",
        description = "Definite integral ∫[lower..upper] f(var) dvar via composite Simpson's rule (10 000 intervals)."
    )]
    fn definite_integral(&self, Parameters(p): Parameters<DefiniteIntegralParams>) -> String {
        calculus::definite_integral(&p.expression, &p.variable, p.lower, p.upper)
    }

    #[tool(
        name = "tangentLine",
        description = "Tangent line to f(var) at a point. Returns JSON {slope, yIntercept, equation}."
    )]
    fn tangent_line(&self, Parameters(p): Parameters<DerivativeParams>) -> String {
        calculus::tangent_line(&p.expression, &p.variable, p.point)
    }

    // ---- Unit converter ------------------------------------------------- //

    #[tool(
        description = "Convert a value between units within an explicit category (e.g. LENGTH, MASS, TEMPERATURE)."
    )]
    fn convert(&self, Parameters(p): Parameters<ConvertParams>) -> String {
        unit_converter::convert(&p.value, &p.from_unit, &p.to_unit, &p.category)
    }

    #[tool(
        name = "convertAutoDetect",
        description = "Convert a value between units, auto-detecting the shared category."
    )]
    fn convert_auto_detect(&self, Parameters(p): Parameters<ConvertAutoParams>) -> String {
        unit_converter::convert_auto_detect(&p.value, &p.from_unit, &p.to_unit)
    }

    // ---- Cooking -------------------------------------------------------- //

    #[tool(
        name = "convertCookingVolume",
        description = "Cooking volume conversion (l/ml/cup/tbsp/tsp/floz/gal). Aliases cup/floz/gal map to US."
    )]
    fn convert_cooking_volume(&self, Parameters(p): Parameters<CookingConvertParams>) -> String {
        cooking::convert_cooking_volume(&p.value, &p.from_unit, &p.to_unit)
    }

    #[tool(
        name = "convertCookingWeight",
        description = "Cooking weight conversion (kg, g, mg, lb, oz)."
    )]
    fn convert_cooking_weight(&self, Parameters(p): Parameters<CookingConvertParams>) -> String {
        cooking::convert_cooking_weight(&p.value, &p.from_unit, &p.to_unit)
    }

    #[tool(
        name = "convertOvenTemperature",
        description = "Oven temperature conversion between Celsius (c), Fahrenheit (f), and UK gas mark (gasmark)."
    )]
    fn convert_oven_temperature(&self, Parameters(p): Parameters<CookingConvertParams>) -> String {
        cooking::convert_oven_temperature(&p.value, &p.from_unit, &p.to_unit)
    }

    // ---- Measure reference --------------------------------------------- //

    #[tool(
        name = "listCategories",
        description = "List every registered measurement category as a JSON array."
    )]
    fn list_categories(&self) -> String {
        measure_reference::list_categories()
    }

    #[tool(
        name = "listUnits",
        description = "List every unit registered in a category. Returns JSON array of {code, name}."
    )]
    fn list_units(&self, Parameters(p): Parameters<CategoryParams>) -> String {
        measure_reference::list_units(&p.category)
    }

    #[tool(
        name = "getConversionFactor",
        description = "Multiplicative factor that maps `fromUnit` to `toUnit` (temperatures use formulas, not a factor)."
    )]
    fn get_conversion_factor(&self, Parameters(p): Parameters<FromToUnitParams>) -> String {
        measure_reference::get_conversion_factor(&p.from_unit, &p.to_unit)
    }

    #[tool(
        name = "explainConversion",
        description = "Human-readable explanation of a unit conversion, e.g. `1 kilometer = 0.621... mile`."
    )]
    fn explain_conversion(&self, Parameters(p): Parameters<FromToUnitParams>) -> String {
        measure_reference::explain_conversion(&p.from_unit, &p.to_unit)
    }

    // ---- DateTime ------------------------------------------------------- //

    #[tool(
        name = "convertTimezone",
        description = "Convert a datetime between IANA timezones. Returns ISO-zoned form with [Zone/ID] suffix."
    )]
    fn convert_timezone(&self, Parameters(p): Parameters<ConvertTimezoneParams>) -> String {
        datetime::convert_timezone(&p.datetime, &p.from_timezone, &p.to_timezone)
    }

    #[tool(
        name = "formatDateTime",
        description = "Reformat a datetime. Format keywords: iso, iso-offset, iso-local, epoch, epochmillis, rfc1123, or strftime."
    )]
    fn format_datetime(&self, Parameters(p): Parameters<FormatDateTimeParams>) -> String {
        datetime::format_datetime(&p.datetime, &p.input_format, &p.output_format, &p.timezone)
    }

    #[tool(
        name = "currentDateTime",
        description = "Current datetime in the given IANA timezone using a format keyword or strftime pattern."
    )]
    fn current_datetime(&self, Parameters(p): Parameters<CurrentDateTimeParams>) -> String {
        datetime::current_datetime(&p.timezone, &p.format)
    }

    #[tool(
        name = "listTimezones",
        description = "JSON array of IANA timezone IDs, filtered by region prefix (e.g. `Europe`, or `all`)."
    )]
    fn list_timezones(&self, Parameters(p): Parameters<ListTimezonesParams>) -> String {
        datetime::list_timezones(&p.region)
    }

    #[tool(
        name = "dateTimeDifference",
        description = "Positive difference between two datetimes in a given zone. Returns JSON with years/months/days/hours/minutes/seconds/totalSeconds."
    )]
    fn datetime_difference(&self, Parameters(p): Parameters<DateTimeDifferenceParams>) -> String {
        datetime::datetime_difference(&p.datetime1, &p.datetime2, &p.timezone)
    }

    // ---- Printing tape -------------------------------------------------- //

    #[tool(
        name = "calculateWithTape",
        description = "Tape calculator: runs a JSON array of {op,value} entries. Ops: + - * / = C T. Returns the printed tape."
    )]
    fn calculate_with_tape(&self, Parameters(p): Parameters<TapeParams>) -> String {
        printing::calculate_with_tape(&p.operations)
    }

    // ---- Graphing ------------------------------------------------------- //

    #[tool(
        name = "plotFunction",
        description = "Sample f(var) over [min, max] with `steps` intervals. Returns JSON array of {x, y}."
    )]
    fn plot_function(&self, Parameters(p): Parameters<PlotFunctionParams>) -> String {
        graphing::plot_function(&p.expression, &p.variable, p.min, p.max, p.steps)
    }

    #[tool(
        name = "solveEquation",
        description = "Solve f(var)=0 near an initial guess using Newton-Raphson. Returns the root or an error."
    )]
    fn solve_equation(&self, Parameters(p): Parameters<SolveEquationParams>) -> String {
        graphing::solve_equation(&p.expression, &p.variable, p.initial_guess)
    }

    #[tool(
        name = "findRoots",
        description = "Find all roots of f(var) in [min, max] via scan + bisection. Returns JSON array of roots."
    )]
    fn find_roots(&self, Parameters(p): Parameters<FindRootsParams>) -> String {
        graphing::find_roots(&p.expression, &p.variable, p.min, p.max)
    }

    // ---- Network -------------------------------------------------------- //

    #[tool(
        name = "subnetCalculator",
        description = "Subnet details (network, broadcast, mask, wildcard, first/last host, usable hosts, IP class) for IPv4 or IPv6."
    )]
    fn subnet_calculator(&self, Parameters(p): Parameters<SubnetParams>) -> String {
        network::subnet_calculator(&p.address, p.cidr)
    }

    #[tool(
        name = "ipToBinary",
        description = "Convert an IPv4/IPv6 address to its binary representation."
    )]
    fn ip_to_binary(&self, Parameters(p): Parameters<AddressParams>) -> String {
        network::ip_to_binary(&p.address)
    }

    #[tool(
        name = "binaryToIp",
        description = "Convert a binary IPv4/IPv6 representation back to decimal notation."
    )]
    fn binary_to_ip(&self, Parameters(p): Parameters<BinaryParams>) -> String {
        network::binary_to_ip(&p.binary)
    }

    #[tool(
        name = "ipToDecimal",
        description = "Convert an IP address to its unsigned decimal integer."
    )]
    fn ip_to_decimal(&self, Parameters(p): Parameters<AddressParams>) -> String {
        network::ip_to_decimal(&p.address)
    }

    #[tool(
        name = "decimalToIp",
        description = "Convert an unsigned decimal integer to an IP address (version must be 4 or 6)."
    )]
    fn decimal_to_ip(&self, Parameters(p): Parameters<DecimalToIpParams>) -> String {
        network::decimal_to_ip(&p.decimal, p.version)
    }

    #[tool(
        name = "ipInSubnet",
        description = "Test whether an IP address belongs to a given subnet. Returns `true` or `false`."
    )]
    fn ip_in_subnet(&self, Parameters(p): Parameters<IpInSubnetParams>) -> String {
        network::ip_in_subnet(&p.address, &p.network, p.cidr)
    }

    #[tool(
        name = "vlsmSubnets",
        description = "VLSM subnet allocation. `hostCounts` is a JSON array of required host counts; returns JSON array of allocated subnets."
    )]
    fn vlsm_subnets(&self, Parameters(p): Parameters<VlsmParams>) -> String {
        network::vlsm_subnets(&p.network_cidr, &p.host_counts)
    }

    #[tool(
        name = "summarizeSubnets",
        description = "Summarize (supernet) a JSON array of IPv4 CIDR blocks into a single CIDR."
    )]
    fn summarize_subnets(&self, Parameters(p): Parameters<SummarizeParams>) -> String {
        network::summarize_subnets(&p.subnets)
    }

    #[tool(
        name = "expandIpv6",
        description = "Expand a compressed IPv6 address to its full 8-group form."
    )]
    fn expand_ipv6(&self, Parameters(p): Parameters<AddressParams>) -> String {
        network::expand_ipv6(&p.address)
    }

    #[tool(
        name = "compressIpv6",
        description = "Compress an IPv6 address to its shortest canonical form using `::`."
    )]
    fn compress_ipv6(&self, Parameters(p): Parameters<AddressParams>) -> String {
        network::compress_ipv6(&p.address)
    }

    #[tool(
        name = "transferTime",
        description = "Estimate file transfer time. Returns JSON with seconds, minutes, hours."
    )]
    fn transfer_time(&self, Parameters(p): Parameters<TransferTimeParams>) -> String {
        network::transfer_time(
            &p.file_size,
            &p.file_size_unit,
            &p.bandwidth,
            &p.bandwidth_unit,
        )
    }

    #[tool(
        description = "Data throughput given data size, elapsed time, and output rate unit (e.g. mbps)."
    )]
    fn throughput(&self, Parameters(p): Parameters<ThroughputParams>) -> String {
        network::throughput(
            &p.data_size,
            &p.data_size_unit,
            &p.time,
            &p.time_unit,
            &p.output_unit,
        )
    }

    #[tool(
        name = "tcpThroughput",
        description = "Effective TCP throughput via bandwidth-delay product. Returns Mbps."
    )]
    fn tcp_throughput(&self, Parameters(p): Parameters<TcpThroughputParams>) -> String {
        network::tcp_throughput(&p.bandwidth_mbps, &p.rtt_ms, &p.window_size_kb)
    }

    // ---- Analog electronics -------------------------------------------- //

    #[tool(
        name = "ohmsLaw",
        description = "Ohm's Law: provide exactly two of V/I/R/P (non-empty strings) and compute the remaining two."
    )]
    fn ohms_law(&self, Parameters(p): Parameters<OhmsLawParams>) -> String {
        analog_electronics::ohms_law(&p.voltage, &p.current, &p.resistance, &p.power)
    }

    #[tool(
        name = "resistorCombination",
        description = "Resistor combination: series sums, parallel reciprocal-sums. Values CSV in ohms."
    )]
    fn resistor_combination(&self, Parameters(p): Parameters<CombinationParams>) -> String {
        analog_electronics::resistor_combination(&p.values, &p.mode)
    }

    #[tool(
        name = "capacitorCombination",
        description = "Capacitor combination: series reciprocal-sums, parallel sums. Values CSV in farads."
    )]
    fn capacitor_combination(&self, Parameters(p): Parameters<CombinationParams>) -> String {
        analog_electronics::capacitor_combination(&p.values, &p.mode)
    }

    #[tool(
        name = "inductorCombination",
        description = "Inductor combination: series sums, parallel reciprocal-sums. Values CSV in henries."
    )]
    fn inductor_combination(&self, Parameters(p): Parameters<CombinationParams>) -> String {
        analog_electronics::inductor_combination(&p.values, &p.mode)
    }

    #[tool(
        name = "voltageDivider",
        description = "Voltage divider: Vout = Vin * R2 / (R1 + R2)."
    )]
    fn voltage_divider(&self, Parameters(p): Parameters<VoltageDividerParams>) -> String {
        analog_electronics::voltage_divider(&p.vin, &p.r1, &p.r2)
    }

    #[tool(
        name = "currentDivider",
        description = "Current divider across two parallel resistors. Returns JSON {i1, i2}."
    )]
    fn current_divider(&self, Parameters(p): Parameters<CurrentDividerParams>) -> String {
        analog_electronics::current_divider(&p.total_current, &p.r1, &p.r2)
    }

    #[tool(
        name = "rcTimeConstant",
        description = "RC time constant τ=RC and cutoff frequency fc=1/(2π·RC). Returns JSON."
    )]
    fn rc_time_constant(&self, Parameters(p): Parameters<RcTimeParams>) -> String {
        analog_electronics::rc_time_constant(&p.resistance, &p.capacitance)
    }

    #[tool(
        name = "rlTimeConstant",
        description = "RL time constant τ=L/R and cutoff frequency fc=R/(2π·L). Returns JSON."
    )]
    fn rl_time_constant(&self, Parameters(p): Parameters<RlTimeParams>) -> String {
        analog_electronics::rl_time_constant(&p.resistance, &p.inductance)
    }

    #[tool(
        name = "rlcResonance",
        description = "RLC resonance: resonant frequency, Q factor, bandwidth. Returns JSON."
    )]
    fn rlc_resonance(&self, Parameters(p): Parameters<RlcParams>) -> String {
        analog_electronics::rlc_resonance(&p.r, &p.l, &p.c)
    }

    #[tool(
        description = "Series RLC impedance magnitude and phase (degrees) at a given frequency. Returns JSON."
    )]
    fn impedance(&self, Parameters(p): Parameters<ImpedanceParams>) -> String {
        analog_electronics::impedance(&p.r, &p.l, &p.c, &p.frequency)
    }

    #[tool(
        name = "decibelConvert",
        description = "Decibel conversion. Mode: powerToDb | voltageToDb | dbToPower | dbToVoltage."
    )]
    fn decibel_convert(&self, Parameters(p): Parameters<DecibelParams>) -> String {
        analog_electronics::decibel_convert(&p.value, &p.mode)
    }

    #[tool(
        name = "filterCutoff",
        description = "RC filter cutoff frequency fc=1/(2π·RC). Filter type: lowpass or highpass."
    )]
    fn filter_cutoff(&self, Parameters(p): Parameters<FilterCutoffParams>) -> String {
        analog_electronics::filter_cutoff(&p.resistance, &p.reactive, &p.filter_type)
    }

    #[tool(
        name = "ledResistor",
        description = "LED current-limiting resistor: R = (Vs - Vf) / If."
    )]
    fn led_resistor(&self, Parameters(p): Parameters<LedResistorParams>) -> String {
        analog_electronics::led_resistor(&p.vs, &p.vf, &p.i_f)
    }

    #[tool(
        name = "wheatstoneBridge",
        description = "Wheatstone bridge balance resistor: R4 = R3·R2 / R1."
    )]
    fn wheatstone_bridge(&self, Parameters(p): Parameters<WheatstoneParams>) -> String {
        analog_electronics::wheatstone_bridge(&p.r1, &p.r2, &p.r3)
    }

    // ---- Digital electronics ------------------------------------------- //

    #[tool(
        name = "convertBase",
        description = "Convert an integer between any two bases in 2..=36. Output is uppercase."
    )]
    fn convert_base(&self, Parameters(p): Parameters<ConvertBaseParams>) -> String {
        digital_electronics::convert_base(&p.value, p.from_base, p.to_base)
    }

    #[tool(
        name = "twosComplement",
        description = "Two's-complement encode (`toTwos`) or decode (`fromTwos`). Bit width 1..=64."
    )]
    fn twos_complement(&self, Parameters(p): Parameters<TwosComplementParams>) -> String {
        digital_electronics::twos_complement(&p.value, p.bits, &p.direction)
    }

    #[tool(
        name = "grayCode",
        description = "Gray-code encode (`toGray`) or decode (`fromGray`) of a binary string."
    )]
    fn gray_code(&self, Parameters(p): Parameters<GrayCodeParams>) -> String {
        digital_electronics::gray_code(&p.value, &p.direction)
    }

    #[tool(
        name = "bitwiseOp",
        description = "Bitwise op: AND, OR, XOR, NOT, SHL, SHR. Returns JSON {decimal, binary}."
    )]
    fn bitwise_op(&self, Parameters(p): Parameters<BitwiseParams>) -> String {
        digital_electronics::bitwise_op(&p.a, &p.b, &p.operation)
    }

    #[tool(
        name = "adcResolution",
        description = "ADC resolution: lsb = Vref / 2^bits, stepCount = 2^bits - 1. Returns JSON."
    )]
    fn adc_resolution(&self, Parameters(p): Parameters<AdcParams>) -> String {
        digital_electronics::adc_resolution(p.bits, &p.vref)
    }

    #[tool(
        name = "dacOutput",
        description = "DAC output voltage: Vout = Vref * code / 2^bits."
    )]
    fn dac_output(&self, Parameters(p): Parameters<DacParams>) -> String {
        digital_electronics::dac_output(p.bits, &p.vref, p.code)
    }

    #[tool(
        name = "timer555Astable",
        description = "555-timer astable: frequency, dutyCycle (%), period. Returns JSON."
    )]
    fn timer_555_astable(&self, Parameters(p): Parameters<Timer555AstableParams>) -> String {
        digital_electronics::timer_555_astable(&p.r1, &p.r2, &p.c)
    }

    #[tool(
        name = "timer555Monostable",
        description = "555-timer monostable pulse width: 1.1·R·C. Returns JSON {pulseWidth}."
    )]
    fn timer_555_monostable(&self, Parameters(p): Parameters<Timer555MonostableParams>) -> String {
        digital_electronics::timer_555_monostable(&p.r, &p.c)
    }

    #[tool(
        name = "frequencyPeriod",
        description = "Convert between frequency and period (reciprocal). Mode: freqToPeriod or periodToFreq."
    )]
    fn frequency_period(&self, Parameters(p): Parameters<FreqPeriodParams>) -> String {
        digital_electronics::frequency_period(&p.value, &p.mode)
    }

    #[tool(
        name = "nyquistRate",
        description = "Nyquist minimum sampling rate: 2 × bandwidth. Returns JSON {minSampleRate, bandwidth}."
    )]
    fn nyquist_rate(&self, Parameters(p): Parameters<NyquistParams>) -> String {
        digital_electronics::nyquist_rate(&p.bandwidth_hz)
    }
}

#[tool_handler]
impl ServerHandler for MathCalcServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::LATEST)
            .with_server_info(Implementation::from_build_env())
            .with_instructions(
                "Pure-Rust math calculator MCP server. 87 tools across 16 categories. \
                 Response format: `TOOL_NAME: OK | KEY: value | ...` (inline) or block layout with `ROW_N` keys for tabular payloads. \
                 Errors: `TOOL_NAME: ERROR` + `REASON: [CODE] text` + optional `DETAIL: k=v`. \
                 Error codes: DOMAIN_ERROR, OUT_OF_RANGE, DIVISION_BY_ZERO, PARSE_ERROR, INVALID_INPUT, UNKNOWN_VARIABLE, UNKNOWN_FUNCTION, OVERFLOW, NOT_IMPLEMENTED. \
                 Categories: \
                 Basic (add, subtract, multiply, divide, power, modulo, abs); \
                 Scientific (sqrt, log, log10, factorial, sin, cos, tan); \
                 Programmable (evaluate, evaluateWithVariables, evaluateExact, evaluateExactWithVariables); \
                 Vector/SIMD (sumArray, dotProduct, scaleArray, magnitudeArray); \
                 Financial (compoundInterest, loanPayment, presentValue, futureValueAnnuity, returnOnInvestment, amortizationSchedule); \
                 Calculus (derivative, nthDerivative, definiteIntegral, tangentLine); \
                 Unit converter (convert, convertAutoDetect); \
                 Cooking (convertCookingVolume, convertCookingWeight, convertOvenTemperature); \
                 Measure reference (listCategories, listUnits, getConversionFactor, explainConversion); \
                 DateTime (convertTimezone, formatDateTime, currentDateTime, listTimezones, dateTimeDifference); \
                 Printing tape (calculateWithTape); \
                 Graphing (plotFunction, solveEquation, findRoots); \
                 Network (subnetCalculator, ipToBinary, binaryToIp, ipToDecimal, decimalToIp, ipInSubnet, vlsmSubnets, summarizeSubnets, expandIpv6, compressIpv6, transferTime, throughput, tcpThroughput); \
                 Analog electronics (ohmsLaw, resistorCombination, capacitorCombination, inductorCombination, voltageDivider, currentDivider, rcTimeConstant, rlTimeConstant, rlcResonance, impedance, decibelConvert, filterCutoff, ledResistor, wheatstoneBridge); \
                 Digital electronics (convertBase, twosComplement, grayCode, bitwiseOp, adcResolution, dacOutput, timer555Astable, timer555Monostable, frequencyPeriod, nyquistRate).",
            )
    }
}
