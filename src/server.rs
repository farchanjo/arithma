//! MCP server: exposes every tool module under `crate::tools` via a single
//! `#[tool_router]` impl block.
//!
//! Every MCP tool method returns `String`; failures are surfaced inline as
//! `"Error: {message}"` so MCP clients always receive a plain text response
//! instead of a protocol-level error.

use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};

use crate::tools::{
    analog_electronics, basic, calculus, chemistry, combinatorics, complex, cooking, crypto,
    datetime, digital_electronics, financial, geometry, graphing, matrices, measure_reference,
    network, physics, printing, programmable, scientific, statistics, unit_converter, vector,
};

#[derive(Clone)]
pub struct MathCalcServer {
    // Accessed both by the `#[tool_handler]` macro (dispatch) and
    // `get_info()` (reports live tool count). The lint is therefore a
    // false positive; the explicit read in `get_info` silences it without
    // requiring `#[allow(dead_code)]`.
    tool_router: ToolRouter<Self>,
}

impl Default for MathCalcServer {
    fn default() -> Self {
        Self::new()
    }
}

// --------------------------------------------------------------------------- //
//  Parameter structs — deduplicated by shape. `#[serde(rename_all = "camelCase")]`
//  is applied to structs with multi-word fields for consistent API naming.
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
    /// Exponent. Integer ≥ 0 uses exact `BigDecimal`; fractional or negative
    /// values fall back to the 128-bit exact evaluator (`2 ^ 0.5`, `2 ^ -3`).
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
    /// Integer in the closed range [0, 1000] — arbitrary-precision output.
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

// ---- statistics ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ValuesParams {
    /// Comma-separated list of finite decimal numbers.
    pub values: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValuesPopulationParams {
    /// Comma-separated list of finite decimal numbers.
    pub values: String,
    /// `true` for population statistic (n denominator); `false` for sample (n-1).
    pub population: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PercentileParams {
    /// Comma-separated list of finite decimal numbers.
    pub values: String,
    /// Percentile in [0, 100].
    pub p: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct QuartileParams {
    /// Comma-separated list of finite decimal numbers.
    pub values: String,
    /// Quartile index: 1, 2, or 3.
    pub q: i32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TwoSeriesParams {
    /// Comma-separated x values.
    pub x_values: String,
    /// Comma-separated y values (same length as `xValues`).
    pub y_values: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CovarianceParams {
    pub x_values: String,
    pub y_values: String,
    pub population: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NormalDistParams {
    pub x: String,
    pub mean: String,
    pub std_dev: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TTestParams {
    pub values: String,
    pub hypothesized_mean: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct BinomialPmfParams {
    /// Number of trials (n >= 0).
    pub n: i64,
    /// Successes count in [0, n].
    pub k: i64,
    /// Success probability in [0, 1].
    pub p: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConfidenceIntervalParams {
    pub values: String,
    /// Confidence level in (0, 1) — e.g. 0.95.
    pub confidence_level: String,
}

// ---- combinatorics ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct NchooseKParams {
    pub n: i64,
    pub k: i64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SingleIntegerParams {
    pub n: i64,
}

// ---- geometry ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RadiusParams {
    pub radius: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct TriangleSidesParams {
    /// Three comma-separated side lengths "a,b,c".
    pub sides: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PolygonCoordsParams {
    /// Vertex coordinates "x1,y1,x2,y2,..." (at least 3 vertices).
    pub coordinates: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RadiusHeightParams {
    pub radius: String,
    pub height: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct TwoPointsParams {
    /// Coordinates of point 1 (CSV: "x,y" or "x,y,z").
    pub p1: String,
    /// Coordinates of point 2 (same dimension as `p1`).
    pub p2: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegularPolygonParams {
    /// Number of sides (>= 3).
    pub sides: i32,
    pub side_length: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PointToLineParams {
    pub point: String,
    pub line_p1: String,
    pub line_p2: String,
}

// ---- complex numbers ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ComplexBinaryParams {
    /// First complex number "real,imag".
    pub a: String,
    /// Second complex number "real,imag".
    pub b: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ComplexUnaryParams {
    /// Complex number "real,imag".
    pub z: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ComplexPowerParams {
    pub z: String,
    pub exponent: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PolarToRectParams {
    pub magnitude: String,
    pub angle_degrees: String,
}

// ---- crypto / encoding ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct StringInputParams {
    /// UTF-8 input text.
    pub input: String,
}

// ---- matrices ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct MatrixParams {
    /// Matrix in row-major form: rows separated by `;`, cells by `,`.
    pub a: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct MatrixBinaryParams {
    pub a: String,
    pub b: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GaussianEliminationParams {
    /// Augmented matrix [A | b] in row-major form (same `;`/`,` syntax).
    pub coefficients: String,
}

// ---- physics ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KinematicsParams {
    pub initial_velocity: String,
    pub acceleration: String,
    pub time: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectileParams {
    pub speed: String,
    pub angle_degrees: String,
    /// Gravitational acceleration in m/s² (Earth ≈ 9.81).
    pub gravity: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct MassAccelParams {
    pub mass: String,
    pub acceleration: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GravitationalForceParams {
    pub m1: String,
    pub m2: String,
    pub distance: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DopplerParams {
    pub source_freq: String,
    pub sound_speed: String,
    pub source_velocity: String,
    pub observer_velocity: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WaveLengthParams {
    pub frequency: String,
    pub wave_speed: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FrequencyParams {
    pub frequency: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IdealGasLawParams {
    pub pressure: String,
    pub volume: String,
    pub moles: String,
    pub temperature: String,
    /// One of "P", "V", "n", "T".
    pub solve_for: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeatTransferParams {
    pub thermal_conductivity: String,
    pub area: String,
    pub delta_temp: String,
    pub thickness: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StefanBoltzmannParams {
    /// Emissivity in [0, 1].
    pub emissivity: String,
    pub area: String,
    pub temperature_k: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct MassRadiusParams {
    pub mass: String,
    pub radius: String,
}

// ---- chemistry ---- //

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FormulaParams {
    /// Chemical formula like "H2O", "Ca(OH)2", "Fe2(SO4)3".
    pub formula: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PhParams {
    pub h_concentration: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PohParams {
    pub oh_concentration: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MolarityParams {
    pub moles: String,
    pub volume_litres: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MolalityParams {
    pub moles: String,
    pub kilograms_solvent: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HendersonHasselbalchParams {
    pub pka: String,
    pub conjugate_base: String,
    pub weak_acid: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DecayParams {
    pub decay_constant: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HalfLifeInputParams {
    pub half_life: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IdealGasMolesParams {
    pub pressure_pa: String,
    pub volume_m3: String,
    pub temperature_k: String,
}

// --------------------------------------------------------------------------- //
//  Tool router — one method per MCP tool.
// --------------------------------------------------------------------------- //

// Handlers delegate straight to module-level functions and carry no shared
// state, so they are written as associated functions — rmcp supports both
// shapes via `SyncAdapter` (no receiver) and `SyncMethodAdapter` (`&self`).
// Using the receiver-less shape keeps clippy happy without `#[allow]`.
#[tool_router]
impl MathCalcServer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    // ---- Basic ---------------------------------------------------------- //

    #[tool(description = "Add two numbers. Returns exact arbitrary-precision result.")]
    fn add(Parameters(p): Parameters<BinaryDecimalParams>) -> String {
        basic::add(&p.first, &p.second)
    }

    #[tool(description = "Subtract second from first. Returns exact arbitrary-precision result.")]
    fn subtract(Parameters(p): Parameters<BinaryDecimalParams>) -> String {
        basic::subtract(&p.first, &p.second)
    }

    #[tool(description = "Multiply two numbers. Returns exact arbitrary-precision result.")]
    fn multiply(Parameters(p): Parameters<BinaryDecimalParams>) -> String {
        basic::multiply(&p.first, &p.second)
    }

    #[tool(description = "Divide first by second. 20-digit precision, HALF_UP rounding.")]
    fn divide(Parameters(p): Parameters<BinaryDecimalParams>) -> String {
        basic::divide(&p.first, &p.second)
    }

    #[tool(
        description = "Raise base to an exponent. Non-negative integer exponents use exact BigDecimal arithmetic; fractional or negative exponents transparently delegate to the 128-bit exact evaluator (so `2 ^ 0.5` and `2 ^ -3` both work)."
    )]
    fn power(Parameters(p): Parameters<PowerParams>) -> String {
        basic::power(&p.base, &p.exponent)
    }

    #[tool(description = "Remainder of first divided by second. Exact result.")]
    fn modulo(Parameters(p): Parameters<BinaryDecimalParams>) -> String {
        basic::modulo(&p.first, &p.second)
    }

    #[tool(description = "Absolute value of a decimal. Exact result.")]
    fn abs(Parameters(p): Parameters<UnaryDecimalParams>) -> String {
        basic::abs(&p.value)
    }

    // ---- Scientific ----------------------------------------------------- //

    #[tool(description = "Square root of a non-negative number.")]
    fn sqrt(Parameters(p): Parameters<UnaryDoubleParams>) -> String {
        scientific::sqrt(p.number)
    }

    #[tool(description = "Natural logarithm (ln) of a positive number.")]
    fn log(Parameters(p): Parameters<UnaryDoubleParams>) -> String {
        scientific::log(p.number)
    }

    #[tool(description = "Base-10 logarithm of a positive number.")]
    fn log10(Parameters(p): Parameters<UnaryDoubleParams>) -> String {
        scientific::log10(p.number)
    }

    #[tool(
        description = "Factorial (n!) for integers in the range 0..=1000. Returns an arbitrary-precision integer — 1000! has 2568 digits."
    )]
    fn factorial(Parameters(p): Parameters<FactorialParams>) -> String {
        scientific::factorial(p.num)
    }

    #[tool(description = "Sine of angle in degrees. Exact at notable angles (0/30/45/60/90/...).")]
    fn sin(Parameters(p): Parameters<AngleParams>) -> String {
        scientific::sin(p.degrees)
    }

    #[tool(
        description = "Cosine of angle in degrees. Exact at notable angles (0/30/45/60/90/...)."
    )]
    fn cos(Parameters(p): Parameters<AngleParams>) -> String {
        scientific::cos(p.degrees)
    }

    #[tool(
        description = "Tangent of angle in degrees. Exact at notable angles; error at 90°/270°."
    )]
    fn tan(Parameters(p): Parameters<AngleParams>) -> String {
        scientific::tan(p.degrees)
    }

    // ---- Programmable --------------------------------------------------- //

    #[tool(
        description = "Evaluate an f64 arithmetic expression. Supports + - * / ^ % and sin/cos/tan (degrees), sin_r/cos_r/tan_r (radians), log/log10/sqrt/abs/ceil/floor. Factorial is spelled `factorial(n)` (not `n!`) and is capped at n <= 20 in this evaluator. Trig is f64-precision so sin(30)*2 ≈ 0.9999... — use evaluateExact or the `sin` tool for notable-angle exactness."
    )]
    fn evaluate(Parameters(p): Parameters<EvaluateParams>) -> String {
        programmable::evaluate(&p.expression)
    }

    #[tool(
        name = "evaluateWithVariables",
        description = "Evaluate an f64 arithmetic expression with a JSON variable map, e.g. expression='2*x+y', variables='{\"x\":3,\"y\":1}'. Variable names pi/e/tau/phi are reserved for engine constants and must not be bound."
    )]
    fn evaluate_with_variables(Parameters(p): Parameters<EvaluateWithVariablesParams>) -> String {
        programmable::evaluate_with_variables(&p.expression, &p.variables)
    }

    #[tool(
        name = "evaluateExact",
        description = "Evaluate an arithmetic expression at 128-bit precision (astro-float). Returns exact decimals (0.1+0.2 = 0.3). Trig functions sin/cos/tan still take degrees — use sin_r/cos_r/tan_r for radians (sin_r(pi) = 0)."
    )]
    fn evaluate_exact(Parameters(p): Parameters<EvaluateParams>) -> String {
        programmable::evaluate_exact(&p.expression)
    }

    #[tool(
        name = "evaluateExactWithVariables",
        description = "Exact evaluator with JSON variable map. Variable values may be numbers or decimal strings; strings preserve >15-digit precision."
    )]
    fn evaluate_exact_with_variables(
        Parameters(p): Parameters<EvaluateWithVariablesParams>,
    ) -> String {
        programmable::evaluate_exact_with_variables(&p.expression, &p.variables)
    }

    // ---- Vector (SIMD) -------------------------------------------------- //

    #[tool(
        name = "sumArray",
        description = "Sum all elements of a comma-separated numeric array (SIMD-accelerated)."
    )]
    fn sum_array(Parameters(p): Parameters<NumbersArrayParams>) -> String {
        vector::sum_array(&p.numbers)
    }

    #[tool(
        name = "dotProduct",
        description = "Dot product of two comma-separated numeric arrays of equal length."
    )]
    fn dot_product(Parameters(p): Parameters<TwoNumberArraysParams>) -> String {
        vector::dot_product(&p.first, &p.second)
    }

    #[tool(
        name = "scaleArray",
        description = "Multiply every element of a CSV numeric array by a scalar; returns CSV."
    )]
    fn scale_array(Parameters(p): Parameters<ScaleArrayParams>) -> String {
        vector::scale_array(&p.numbers, &p.scalar)
    }

    #[tool(
        name = "magnitudeArray",
        description = "Euclidean magnitude (L2 norm) of a CSV numeric vector."
    )]
    fn magnitude_array(Parameters(p): Parameters<NumbersArrayParams>) -> String {
        vector::magnitude_array(&p.numbers)
    }

    // ---- Financial ------------------------------------------------------ //

    #[tool(
        name = "compoundInterest",
        description = "Future value with compound interest: A = P*(1 + r/n)^(n*t). Rate is a percent (e.g. 5 = 5%)."
    )]
    fn compound_interest(Parameters(p): Parameters<CompoundInterestParams>) -> String {
        financial::compound_interest(&p.principal, &p.annual_rate, &p.years, p.compounds_per_year)
    }

    #[tool(
        name = "loanPayment",
        description = "Fixed monthly payment for an amortizing loan given principal, annual rate (%) and years."
    )]
    fn loan_payment(Parameters(p): Parameters<LoanPaymentParams>) -> String {
        financial::loan_payment(&p.principal, &p.annual_rate, &p.years)
    }

    #[tool(
        name = "presentValue",
        description = "Present value of a future amount: PV = FV / (1 + r)^t. Rate is a percent."
    )]
    fn present_value(Parameters(p): Parameters<PresentValueParams>) -> String {
        financial::present_value(&p.future_value, &p.annual_rate, &p.years)
    }

    #[tool(
        name = "futureValueAnnuity",
        description = "Future value of an ordinary annuity: FV = PMT * ((1+r)^n - 1) / r."
    )]
    fn future_value_annuity(Parameters(p): Parameters<FutureValueAnnuityParams>) -> String {
        financial::future_value_annuity(&p.payment, &p.annual_rate, &p.years)
    }

    #[tool(
        name = "returnOnInvestment",
        description = "Return on investment as a percentage: ROI = (gain - cost) / cost * 100."
    )]
    fn return_on_investment(Parameters(p): Parameters<RoiParams>) -> String {
        financial::return_on_investment(&p.gain, &p.cost)
    }

    #[tool(
        name = "amortizationSchedule",
        description = "Monthly amortization schedule as a JSON array of {month, payment, principal, interest, balance}."
    )]
    fn amortization_schedule(Parameters(p): Parameters<LoanPaymentParams>) -> String {
        financial::amortization_schedule(&p.principal, &p.annual_rate, &p.years)
    }

    // ---- Calculus ------------------------------------------------------- //

    #[tool(
        description = "Numerical derivative (five-point central difference) of an expression at a point."
    )]
    fn derivative(Parameters(p): Parameters<DerivativeParams>) -> String {
        calculus::derivative(&p.expression, &p.variable, p.point)
    }

    #[tool(
        name = "nthDerivative",
        description = "Nth-order numerical derivative of an expression at a point. Order must be in [1, 10]. High orders amplify the f64 truncation error: d^10/dx^10 x^10 at x=1 returns 3628800.000000006 instead of the exact 10! = 3628800."
    )]
    fn nth_derivative(Parameters(p): Parameters<NthDerivativeParams>) -> String {
        calculus::nth_derivative(&p.expression, &p.variable, p.point, p.order)
    }

    #[tool(
        name = "definiteIntegral",
        description = "Definite integral ∫[lower..upper] f(var) dvar via composite Simpson's rule (10 000 intervals). Trig functions inside the integrand (sin/cos/tan) take DEGREES by default — use sin_r/cos_r/tan_r if you want radians (e.g. ∫₀^π sin_r(x) dx = 2)."
    )]
    fn definite_integral(Parameters(p): Parameters<DefiniteIntegralParams>) -> String {
        calculus::definite_integral(&p.expression, &p.variable, p.lower, p.upper)
    }

    #[tool(
        name = "tangentLine",
        description = "Tangent line to f(var) at a point. Returns JSON {slope, yIntercept, equation}."
    )]
    fn tangent_line(Parameters(p): Parameters<DerivativeParams>) -> String {
        calculus::tangent_line(&p.expression, &p.variable, p.point)
    }

    // ---- Unit converter ------------------------------------------------- //

    #[tool(
        description = "Convert a value between units within an explicit category (e.g. LENGTH, MASS, TEMPERATURE)."
    )]
    fn convert(Parameters(p): Parameters<ConvertParams>) -> String {
        unit_converter::convert(&p.value, &p.from_unit, &p.to_unit, &p.category)
    }

    #[tool(
        name = "convertAutoDetect",
        description = "Convert a value between units, auto-detecting the shared category."
    )]
    fn convert_auto_detect(Parameters(p): Parameters<ConvertAutoParams>) -> String {
        unit_converter::convert_auto_detect(&p.value, &p.from_unit, &p.to_unit)
    }

    // ---- Cooking -------------------------------------------------------- //

    #[tool(
        name = "convertCookingVolume",
        description = "Cooking volume conversion (l/ml/cup/tbsp/tsp/floz/gal). Aliases cup/floz/gal map to US."
    )]
    fn convert_cooking_volume(Parameters(p): Parameters<CookingConvertParams>) -> String {
        cooking::convert_cooking_volume(&p.value, &p.from_unit, &p.to_unit)
    }

    #[tool(
        name = "convertCookingWeight",
        description = "Cooking weight conversion (kg, g, mg, lb, oz)."
    )]
    fn convert_cooking_weight(Parameters(p): Parameters<CookingConvertParams>) -> String {
        cooking::convert_cooking_weight(&p.value, &p.from_unit, &p.to_unit)
    }

    #[tool(
        name = "convertOvenTemperature",
        description = "Oven temperature conversion between Celsius (c), Fahrenheit (f), and UK gas mark (gasmark)."
    )]
    fn convert_oven_temperature(Parameters(p): Parameters<CookingConvertParams>) -> String {
        cooking::convert_oven_temperature(&p.value, &p.from_unit, &p.to_unit)
    }

    // ---- Measure reference --------------------------------------------- //

    #[tool(
        name = "listCategories",
        description = "List every registered MEASUREMENT UNIT category (e.g. LENGTH, MASS, TEMPERATURE). These are the 21 categories consumed by `convert`, `listUnits`, and friends. Distinct from the 23 TOOL MODULE categories surfaced in the server instructions (Basic, Scientific, ...) — use `listToolCategories` for those."
    )]
    fn list_categories() -> String {
        measure_reference::list_categories()
    }

    #[tool(
        name = "listToolCategories",
        description = "List the 23 tool-module categories that organize arithma's tools (Basic, Scientific, Programmable, Vector/SIMD, Financial, ...). Distinct from `listCategories`, which returns the 21 measurement-unit categories consumed by the unit-converter tools."
    )]
    fn list_tool_categories() -> String {
        measure_reference::list_tool_categories()
    }

    #[tool(
        name = "listUnits",
        description = "List every unit registered in a category. Returns JSON array of {code, name}."
    )]
    fn list_units(Parameters(p): Parameters<CategoryParams>) -> String {
        measure_reference::list_units(&p.category)
    }

    #[tool(
        name = "getConversionFactor",
        description = "Multiplicative factor that maps `fromUnit` to `toUnit` (temperatures use formulas, not a factor)."
    )]
    fn get_conversion_factor(Parameters(p): Parameters<FromToUnitParams>) -> String {
        measure_reference::get_conversion_factor(&p.from_unit, &p.to_unit)
    }

    #[tool(
        name = "explainConversion",
        description = "Human-readable explanation of a unit conversion, e.g. `1 kilometer = 0.621... mile`."
    )]
    fn explain_conversion(Parameters(p): Parameters<FromToUnitParams>) -> String {
        measure_reference::explain_conversion(&p.from_unit, &p.to_unit)
    }

    // ---- DateTime ------------------------------------------------------- //

    #[tool(
        name = "convertTimezone",
        description = "Convert a datetime between IANA timezones. Returns ISO-zoned form with [Zone/ID] suffix."
    )]
    fn convert_timezone(Parameters(p): Parameters<ConvertTimezoneParams>) -> String {
        datetime::convert_timezone(&p.datetime, &p.from_timezone, &p.to_timezone)
    }

    #[tool(
        name = "formatDateTime",
        description = "Reformat a datetime. Format keywords: iso, iso-offset, iso-local, epoch, epochmillis, rfc1123, or strftime."
    )]
    fn format_datetime(Parameters(p): Parameters<FormatDateTimeParams>) -> String {
        datetime::format_datetime(&p.datetime, &p.input_format, &p.output_format, &p.timezone)
    }

    #[tool(
        name = "currentDateTime",
        description = "Current datetime in the given IANA timezone using a format keyword or strftime pattern."
    )]
    fn current_datetime(Parameters(p): Parameters<CurrentDateTimeParams>) -> String {
        datetime::current_datetime(&p.timezone, &p.format)
    }

    #[tool(
        name = "listTimezones",
        description = "JSON array of IANA timezone IDs, filtered by region prefix (e.g. `Europe`, or `all`)."
    )]
    fn list_timezones(Parameters(p): Parameters<ListTimezonesParams>) -> String {
        datetime::list_timezones(&p.region)
    }

    #[tool(
        name = "dateTimeDifference",
        description = "Positive difference between two datetimes in a given zone. Returns JSON with years/months/days/hours/minutes/seconds/totalSeconds."
    )]
    fn datetime_difference(Parameters(p): Parameters<DateTimeDifferenceParams>) -> String {
        datetime::datetime_difference(&p.datetime1, &p.datetime2, &p.timezone)
    }

    // ---- Printing tape -------------------------------------------------- //

    #[tool(
        name = "calculateWithTape",
        description = "Tape calculator: runs a JSON array of {op,value} entries. Ops: + - * / = C T. Returns the printed tape."
    )]
    fn calculate_with_tape(Parameters(p): Parameters<TapeParams>) -> String {
        printing::calculate_with_tape(&p.operations)
    }

    // ---- Graphing ------------------------------------------------------- //

    #[tool(
        name = "plotFunction",
        description = "Sample f(var) over [min, max] with `steps` intervals. Returns JSON array of {x, y}."
    )]
    fn plot_function(Parameters(p): Parameters<PlotFunctionParams>) -> String {
        graphing::plot_function(&p.expression, &p.variable, p.min, p.max, p.steps)
    }

    #[tool(
        name = "solveEquation",
        description = "Solve f(var)=0 near an initial guess using Newton-Raphson. Returns the root or an error."
    )]
    fn solve_equation(Parameters(p): Parameters<SolveEquationParams>) -> String {
        graphing::solve_equation(&p.expression, &p.variable, p.initial_guess)
    }

    #[tool(
        name = "findRoots",
        description = "Find all roots of f(var) in [min, max] via scan + bisection. Returns JSON array of roots."
    )]
    fn find_roots(Parameters(p): Parameters<FindRootsParams>) -> String {
        graphing::find_roots(&p.expression, &p.variable, p.min, p.max)
    }

    // ---- Network -------------------------------------------------------- //

    #[tool(
        name = "subnetCalculator",
        description = "Subnet details (network, broadcast, mask, wildcard, first/last host, usable hosts, IP class) for IPv4 or IPv6. /31 returns 2 usable hosts per RFC 3021 (point-to-point links); /32 returns 1 (host route)."
    )]
    fn subnet_calculator(Parameters(p): Parameters<SubnetParams>) -> String {
        network::subnet_calculator(&p.address, p.cidr)
    }

    #[tool(
        name = "ipToBinary",
        description = "Convert an IPv4/IPv6 address to its binary representation."
    )]
    fn ip_to_binary(Parameters(p): Parameters<AddressParams>) -> String {
        network::ip_to_binary(&p.address)
    }

    #[tool(
        name = "binaryToIp",
        description = "Convert a binary IPv4/IPv6 representation back to decimal notation."
    )]
    fn binary_to_ip(Parameters(p): Parameters<BinaryParams>) -> String {
        network::binary_to_ip(&p.binary)
    }

    #[tool(
        name = "ipToDecimal",
        description = "Convert an IP address to its unsigned decimal integer."
    )]
    fn ip_to_decimal(Parameters(p): Parameters<AddressParams>) -> String {
        network::ip_to_decimal(&p.address)
    }

    #[tool(
        name = "decimalToIp",
        description = "Convert an unsigned decimal integer to an IP address (version must be 4 or 6)."
    )]
    fn decimal_to_ip(Parameters(p): Parameters<DecimalToIpParams>) -> String {
        network::decimal_to_ip(&p.decimal, p.version)
    }

    #[tool(
        name = "ipInSubnet",
        description = "Test whether an IP address belongs to a given subnet. Returns `true` or `false`."
    )]
    fn ip_in_subnet(Parameters(p): Parameters<IpInSubnetParams>) -> String {
        network::ip_in_subnet(&p.address, &p.network, p.cidr)
    }

    #[tool(
        name = "vlsmSubnets",
        description = "VLSM subnet allocation. `hostCounts` is a JSON array of required host counts; returns JSON array of allocated subnets."
    )]
    fn vlsm_subnets(Parameters(p): Parameters<VlsmParams>) -> String {
        network::vlsm_subnets(&p.network_cidr, &p.host_counts)
    }

    #[tool(
        name = "summarizeSubnets",
        description = "Summarize (supernet) a JSON array of IPv4 CIDR blocks into a single CIDR."
    )]
    fn summarize_subnets(Parameters(p): Parameters<SummarizeParams>) -> String {
        network::summarize_subnets(&p.subnets)
    }

    #[tool(
        name = "expandIpv6",
        description = "Expand a compressed IPv6 address to its full 8-group form."
    )]
    fn expand_ipv6(Parameters(p): Parameters<AddressParams>) -> String {
        network::expand_ipv6(&p.address)
    }

    #[tool(
        name = "compressIpv6",
        description = "Compress an IPv6 address to its shortest canonical form using `::`."
    )]
    fn compress_ipv6(Parameters(p): Parameters<AddressParams>) -> String {
        network::compress_ipv6(&p.address)
    }

    #[tool(
        name = "transferTime",
        description = "Estimate file transfer time. Returns JSON with seconds, minutes, hours."
    )]
    fn transfer_time(Parameters(p): Parameters<TransferTimeParams>) -> String {
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
    fn throughput(Parameters(p): Parameters<ThroughputParams>) -> String {
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
        description = "Effective TCP throughput via bandwidth-delay product. Returns Mbps. Uses SI units: 1 kB = 1000 bytes (not 1024); 1 Mbps = 10^6 bps. Matches the DATA_STORAGE/DATA_RATE unit registry."
    )]
    fn tcp_throughput(Parameters(p): Parameters<TcpThroughputParams>) -> String {
        network::tcp_throughput(&p.bandwidth_mbps, &p.rtt_ms, &p.window_size_kb)
    }

    // ---- Analog electronics -------------------------------------------- //

    #[tool(
        name = "ohmsLaw",
        description = "Ohm's Law: provide exactly two of V/I/R/P (non-empty strings) and compute the remaining two."
    )]
    fn ohms_law(Parameters(p): Parameters<OhmsLawParams>) -> String {
        analog_electronics::ohms_law(&p.voltage, &p.current, &p.resistance, &p.power)
    }

    #[tool(
        name = "resistorCombination",
        description = "Resistor combination: series sums, parallel reciprocal-sums. Values CSV in ohms."
    )]
    fn resistor_combination(Parameters(p): Parameters<CombinationParams>) -> String {
        analog_electronics::resistor_combination(&p.values, &p.mode)
    }

    #[tool(
        name = "capacitorCombination",
        description = "Capacitor combination: series reciprocal-sums, parallel sums. Values CSV in farads."
    )]
    fn capacitor_combination(Parameters(p): Parameters<CombinationParams>) -> String {
        analog_electronics::capacitor_combination(&p.values, &p.mode)
    }

    #[tool(
        name = "inductorCombination",
        description = "Inductor combination: series sums, parallel reciprocal-sums. Values CSV in henries."
    )]
    fn inductor_combination(Parameters(p): Parameters<CombinationParams>) -> String {
        analog_electronics::inductor_combination(&p.values, &p.mode)
    }

    #[tool(
        name = "voltageDivider",
        description = "Voltage divider: Vout = Vin * R2 / (R1 + R2)."
    )]
    fn voltage_divider(Parameters(p): Parameters<VoltageDividerParams>) -> String {
        analog_electronics::voltage_divider(&p.vin, &p.r1, &p.r2)
    }

    #[tool(
        name = "currentDivider",
        description = "Current divider across two parallel resistors. Returns JSON {i1, i2}."
    )]
    fn current_divider(Parameters(p): Parameters<CurrentDividerParams>) -> String {
        analog_electronics::current_divider(&p.total_current, &p.r1, &p.r2)
    }

    #[tool(
        name = "rcTimeConstant",
        description = "RC time constant τ=RC and cutoff frequency fc=1/(2π·RC). Returns JSON."
    )]
    fn rc_time_constant(Parameters(p): Parameters<RcTimeParams>) -> String {
        analog_electronics::rc_time_constant(&p.resistance, &p.capacitance)
    }

    #[tool(
        name = "rlTimeConstant",
        description = "RL time constant τ=L/R and cutoff frequency fc=R/(2π·L). Returns JSON."
    )]
    fn rl_time_constant(Parameters(p): Parameters<RlTimeParams>) -> String {
        analog_electronics::rl_time_constant(&p.resistance, &p.inductance)
    }

    #[tool(
        name = "rlcResonance",
        description = "RLC resonance: resonant frequency, Q factor, bandwidth. Returns JSON."
    )]
    fn rlc_resonance(Parameters(p): Parameters<RlcParams>) -> String {
        analog_electronics::rlc_resonance(&p.r, &p.l, &p.c)
    }

    #[tool(
        description = "Series RLC impedance magnitude and phase (degrees) at a given frequency. Returns JSON."
    )]
    fn impedance(Parameters(p): Parameters<ImpedanceParams>) -> String {
        analog_electronics::impedance(&p.r, &p.l, &p.c, &p.frequency)
    }

    #[tool(
        name = "decibelConvert",
        description = "Decibel conversion. Mode: powerToDb | voltageToDb | dbToPower | dbToVoltage."
    )]
    fn decibel_convert(Parameters(p): Parameters<DecibelParams>) -> String {
        analog_electronics::decibel_convert(&p.value, &p.mode)
    }

    #[tool(
        name = "filterCutoff",
        description = "RC filter cutoff frequency fc=1/(2π·RC). Filter type: lowpass or highpass."
    )]
    fn filter_cutoff(Parameters(p): Parameters<FilterCutoffParams>) -> String {
        analog_electronics::filter_cutoff(&p.resistance, &p.reactive, &p.filter_type)
    }

    #[tool(
        name = "ledResistor",
        description = "LED current-limiting resistor: R = (Vs - Vf) / If."
    )]
    fn led_resistor(Parameters(p): Parameters<LedResistorParams>) -> String {
        analog_electronics::led_resistor(&p.vs, &p.vf, &p.i_f)
    }

    #[tool(
        name = "wheatstoneBridge",
        description = "Wheatstone bridge balance resistor: R4 = R3·R2 / R1."
    )]
    fn wheatstone_bridge(Parameters(p): Parameters<WheatstoneParams>) -> String {
        analog_electronics::wheatstone_bridge(&p.r1, &p.r2, &p.r3)
    }

    // ---- Digital electronics ------------------------------------------- //

    #[tool(
        name = "convertBase",
        description = "Convert an integer between any two bases in 2..=36. Output is uppercase."
    )]
    fn convert_base(Parameters(p): Parameters<ConvertBaseParams>) -> String {
        digital_electronics::convert_base(&p.value, p.from_base, p.to_base)
    }

    #[tool(
        name = "twosComplement",
        description = "Two's-complement encode (`toTwos`) or decode (`fromTwos`). Bit width 1..=64."
    )]
    fn twos_complement(Parameters(p): Parameters<TwosComplementParams>) -> String {
        digital_electronics::twos_complement(&p.value, p.bits, &p.direction)
    }

    #[tool(
        name = "grayCode",
        description = "Gray-code encode (`toGray`) or decode (`fromGray`) of a binary string."
    )]
    fn gray_code(Parameters(p): Parameters<GrayCodeParams>) -> String {
        digital_electronics::gray_code(&p.value, &p.direction)
    }

    #[tool(
        name = "bitwiseOp",
        description = "Bitwise op: AND, OR, XOR, NOT, SHL, SHR. Operates on arbitrary-precision signed integers (two's-complement with unbounded width): NOT n = -(n+1), so NOT 5 = -6; shift counts are u32, and results are not truncated to any fixed bit-width. For fixed-width two's-complement work use `twosComplement`."
    )]
    fn bitwise_op(Parameters(p): Parameters<BitwiseParams>) -> String {
        digital_electronics::bitwise_op(&p.a, &p.b, &p.operation)
    }

    #[tool(
        name = "adcResolution",
        description = "ADC resolution: lsb = Vref / 2^bits, stepCount = 2^bits - 1. Returns JSON."
    )]
    fn adc_resolution(Parameters(p): Parameters<AdcParams>) -> String {
        digital_electronics::adc_resolution(p.bits, &p.vref)
    }

    #[tool(
        name = "dacOutput",
        description = "DAC output voltage: Vout = Vref * code / 2^bits."
    )]
    fn dac_output(Parameters(p): Parameters<DacParams>) -> String {
        digital_electronics::dac_output(p.bits, &p.vref, p.code)
    }

    #[tool(
        name = "timer555Astable",
        description = "555-timer astable: frequency, dutyCycle (%), period. Returns JSON."
    )]
    fn timer_555_astable(Parameters(p): Parameters<Timer555AstableParams>) -> String {
        digital_electronics::timer_555_astable(&p.r1, &p.r2, &p.c)
    }

    #[tool(
        name = "timer555Monostable",
        description = "555-timer monostable pulse width: 1.1·R·C. Returns JSON {pulseWidth}."
    )]
    fn timer_555_monostable(Parameters(p): Parameters<Timer555MonostableParams>) -> String {
        digital_electronics::timer_555_monostable(&p.r, &p.c)
    }

    #[tool(
        name = "frequencyPeriod",
        description = "Convert between frequency and period (reciprocal). Mode: freqToPeriod or periodToFreq."
    )]
    fn frequency_period(Parameters(p): Parameters<FreqPeriodParams>) -> String {
        digital_electronics::frequency_period(&p.value, &p.mode)
    }

    #[tool(
        name = "nyquistRate",
        description = "Nyquist minimum sampling rate: 2 × bandwidth. Returns JSON {minSampleRate, bandwidth}."
    )]
    fn nyquist_rate(Parameters(p): Parameters<NyquistParams>) -> String {
        digital_electronics::nyquist_rate(&p.bandwidth_hz)
    }

    // ---- Statistics ----------------------------------------------------- //

    #[tool(
        name = "mean",
        description = "Arithmetic mean of a comma-separated array."
    )]
    fn mean(Parameters(p): Parameters<ValuesParams>) -> String {
        statistics::mean(&p.values)
    }

    #[tool(
        name = "median",
        description = "Median (middle value) of an array; averages middles for even-length input."
    )]
    fn median(Parameters(p): Parameters<ValuesParams>) -> String {
        statistics::median(&p.values)
    }

    #[tool(
        name = "mode",
        description = "Mode(s) of an array — most frequent value(s) with their count."
    )]
    fn mode(Parameters(p): Parameters<ValuesParams>) -> String {
        statistics::mode(&p.values)
    }

    #[tool(
        name = "variance",
        description = "Variance of an array. Set population=true for n denominator, false for n-1 (sample)."
    )]
    fn variance(Parameters(p): Parameters<ValuesPopulationParams>) -> String {
        statistics::variance(&p.values, p.population)
    }

    #[tool(
        name = "stdDev",
        description = "Standard deviation. population=true → σ, false → s."
    )]
    fn std_dev(Parameters(p): Parameters<ValuesPopulationParams>) -> String {
        statistics::std_dev(&p.values, p.population)
    }

    #[tool(
        name = "percentile",
        description = "Linear-interpolated percentile (R-7/Excel definition). p in [0, 100]."
    )]
    fn percentile(Parameters(p): Parameters<PercentileParams>) -> String {
        statistics::percentile(&p.values, &p.p)
    }

    #[tool(
        name = "quartile",
        description = "Quartile Q1, Q2, or Q3 (q in [1, 3])."
    )]
    fn quartile(Parameters(p): Parameters<QuartileParams>) -> String {
        statistics::quartile(&p.values, p.q)
    }

    #[tool(
        name = "iqr",
        description = "Interquartile range Q3 - Q1 (also returns Q1 and Q3)."
    )]
    fn iqr(Parameters(p): Parameters<ValuesParams>) -> String {
        statistics::iqr(&p.values)
    }

    #[tool(
        name = "correlation",
        description = "Pearson correlation coefficient between two equal-length series."
    )]
    fn correlation(Parameters(p): Parameters<TwoSeriesParams>) -> String {
        statistics::correlation(&p.x_values, &p.y_values)
    }

    #[tool(
        name = "covariance",
        description = "Sample (population=false) or population covariance between two series."
    )]
    fn covariance(Parameters(p): Parameters<CovarianceParams>) -> String {
        statistics::covariance(&p.x_values, &p.y_values, p.population)
    }

    #[tool(
        name = "linearRegression",
        description = "Ordinary least-squares fit y = a*x + b. Returns slope, intercept, R, R²."
    )]
    fn linear_regression(Parameters(p): Parameters<TwoSeriesParams>) -> String {
        statistics::linear_regression(&p.x_values, &p.y_values)
    }

    #[tool(
        name = "normalPdf",
        description = "Normal distribution PDF: f(x; mean, stdDev)."
    )]
    fn normal_pdf(Parameters(p): Parameters<NormalDistParams>) -> String {
        statistics::normal_pdf(&p.x, &p.mean, &p.std_dev)
    }

    #[tool(
        name = "normalCdf",
        description = "Normal distribution CDF using erf approximation (max error ~1.5e-7)."
    )]
    fn normal_cdf(Parameters(p): Parameters<NormalDistParams>) -> String {
        statistics::normal_cdf(&p.x, &p.mean, &p.std_dev)
    }

    #[tool(
        name = "tTestOneSample",
        description = "One-sample t-test against a hypothesized mean. Returns t, df, mean, SE."
    )]
    fn t_test_one_sample(Parameters(p): Parameters<TTestParams>) -> String {
        statistics::t_test_one_sample(&p.values, &p.hypothesized_mean)
    }

    #[tool(
        name = "binomialPmf",
        description = "Binomial PMF B(n, p) at k. Capped at n=1000."
    )]
    fn binomial_pmf(Parameters(p): Parameters<BinomialPmfParams>) -> String {
        statistics::binomial_pmf(p.n, p.k, &p.p)
    }

    #[tool(
        name = "confidenceInterval",
        description = "Two-sided confidence interval for the mean (z-score)."
    )]
    fn confidence_interval(Parameters(p): Parameters<ConfidenceIntervalParams>) -> String {
        statistics::confidence_interval(&p.values, &p.confidence_level)
    }

    // ---- Combinatorics & number theory --------------------------------- //

    #[tool(
        name = "combination",
        description = "Binomial coefficient C(n, k) — exact arbitrary precision."
    )]
    fn combination(Parameters(p): Parameters<NchooseKParams>) -> String {
        combinatorics::combination(p.n, p.k)
    }

    #[tool(
        name = "permutation",
        description = "Falling factorial P(n, k) = n!/(n-k)! — exact."
    )]
    fn permutation(Parameters(p): Parameters<NchooseKParams>) -> String {
        combinatorics::permutation(p.n, p.k)
    }

    #[tool(
        name = "fibonacci",
        description = "F(n) — exact arbitrary precision (n <= 50000)."
    )]
    fn fibonacci(Parameters(p): Parameters<SingleIntegerParams>) -> String {
        combinatorics::fibonacci(p.n)
    }

    #[tool(
        name = "isPrime",
        description = "Primality test via trial division (works up to ~10^18)."
    )]
    fn is_prime(Parameters(p): Parameters<SingleIntegerParams>) -> String {
        combinatorics::is_prime(p.n)
    }

    #[tool(
        name = "nextPrime",
        description = "Smallest prime strictly greater than n."
    )]
    fn next_prime(Parameters(p): Parameters<SingleIntegerParams>) -> String {
        combinatorics::next_prime(p.n)
    }

    #[tool(
        name = "primeFactors",
        description = "Multiset of prime factors of n via trial division (n <= 10^12)."
    )]
    fn prime_factors(Parameters(p): Parameters<SingleIntegerParams>) -> String {
        combinatorics::prime_factors(p.n)
    }

    #[tool(
        name = "eulerTotient",
        description = "Euler's totient φ(n): count of integers in [1,n] coprime to n."
    )]
    fn euler_totient(Parameters(p): Parameters<SingleIntegerParams>) -> String {
        combinatorics::euler_totient(p.n)
    }

    // ---- Geometry ------------------------------------------------------ //

    #[tool(name = "circleArea", description = "Area of a circle: π·r².")]
    fn circle_area(Parameters(p): Parameters<RadiusParams>) -> String {
        geometry::circle_area(&p.radius)
    }

    #[tool(
        name = "circlePerimeter",
        description = "Circumference of a circle: 2π·r."
    )]
    fn circle_perimeter(Parameters(p): Parameters<RadiusParams>) -> String {
        geometry::circle_perimeter(&p.radius)
    }

    #[tool(name = "sphereVolume", description = "Volume of a sphere: 4π·r³/3.")]
    fn sphere_volume(Parameters(p): Parameters<RadiusParams>) -> String {
        geometry::sphere_volume(&p.radius)
    }

    #[tool(name = "sphereArea", description = "Surface area of a sphere: 4π·r².")]
    fn sphere_area(Parameters(p): Parameters<RadiusParams>) -> String {
        geometry::sphere_area(&p.radius)
    }

    #[tool(
        name = "triangleArea",
        description = "Triangle area via Heron's formula. sides=\"a,b,c\"."
    )]
    fn triangle_area(Parameters(p): Parameters<TriangleSidesParams>) -> String {
        geometry::triangle_area(&p.sides)
    }

    #[tool(
        name = "polygonArea",
        description = "Polygon area via Shoelace formula. coordinates=\"x1,y1,x2,y2,...\"."
    )]
    fn polygon_area(Parameters(p): Parameters<PolygonCoordsParams>) -> String {
        geometry::polygon_area(&p.coordinates)
    }

    #[tool(name = "coneVolume", description = "Cone volume: π·r²·h/3.")]
    fn cone_volume(Parameters(p): Parameters<RadiusHeightParams>) -> String {
        geometry::cone_volume(&p.radius, &p.height)
    }

    #[tool(name = "cylinderVolume", description = "Cylinder volume: π·r²·h.")]
    fn cylinder_volume(Parameters(p): Parameters<RadiusHeightParams>) -> String {
        geometry::cylinder_volume(&p.radius, &p.height)
    }

    #[tool(
        name = "distance2D",
        description = "Euclidean distance between two 2D points."
    )]
    fn distance_2d(Parameters(p): Parameters<TwoPointsParams>) -> String {
        geometry::distance_2d(&p.p1, &p.p2)
    }

    #[tool(
        name = "distance3D",
        description = "Euclidean distance between two 3D points."
    )]
    fn distance_3d(Parameters(p): Parameters<TwoPointsParams>) -> String {
        geometry::distance_3d(&p.p1, &p.p2)
    }

    #[tool(
        name = "regularPolygon",
        description = "Area, perimeter, apothem, circumradius for a regular n-gon."
    )]
    fn regular_polygon(Parameters(p): Parameters<RegularPolygonParams>) -> String {
        geometry::regular_polygon(p.sides, &p.side_length)
    }

    #[tool(
        name = "pointToLineDistance",
        description = "Perpendicular distance from a 2D point to a line through two points."
    )]
    fn point_to_line_distance(Parameters(p): Parameters<PointToLineParams>) -> String {
        geometry::point_to_line_distance(&p.point, &p.line_p1, &p.line_p2)
    }

    // ---- Complex numbers ----------------------------------------------- //

    #[tool(
        name = "complexAdd",
        description = "Add complex numbers a + b. Inputs are \"real,imag\" CSV."
    )]
    fn complex_add(Parameters(p): Parameters<ComplexBinaryParams>) -> String {
        complex::complex_add(&p.a, &p.b)
    }

    #[tool(name = "complexMult", description = "Multiply complex numbers a × b.")]
    fn complex_mult(Parameters(p): Parameters<ComplexBinaryParams>) -> String {
        complex::complex_mult(&p.a, &p.b)
    }

    #[tool(name = "complexDiv", description = "Divide complex numbers a / b.")]
    fn complex_div(Parameters(p): Parameters<ComplexBinaryParams>) -> String {
        complex::complex_div(&p.a, &p.b)
    }

    #[tool(
        name = "complexConjugate",
        description = "Complex conjugate of z (flips sign of imaginary part)."
    )]
    fn complex_conjugate(Parameters(p): Parameters<ComplexUnaryParams>) -> String {
        complex::complex_conjugate(&p.z)
    }

    #[tool(
        name = "complexPower",
        description = "z^n for real exponent via De Moivre."
    )]
    fn complex_power(Parameters(p): Parameters<ComplexPowerParams>) -> String {
        complex::complex_power(&p.z, &p.exponent)
    }

    #[tool(
        name = "complexMagnitude",
        description = "Magnitude |z| = sqrt(real² + imag²)."
    )]
    fn complex_magnitude(Parameters(p): Parameters<ComplexUnaryParams>) -> String {
        complex::complex_magnitude(&p.z)
    }

    #[tool(
        name = "complexPhase",
        description = "Phase angle in degrees, range (-180, 180]."
    )]
    fn complex_phase(Parameters(p): Parameters<ComplexUnaryParams>) -> String {
        complex::complex_phase(&p.z)
    }

    #[tool(
        name = "polarToRect",
        description = "Polar (magnitude, angleDegrees) → rectangular (real, imag)."
    )]
    fn polar_to_rect(Parameters(p): Parameters<PolarToRectParams>) -> String {
        complex::polar_to_rect(&p.magnitude, &p.angle_degrees)
    }

    #[tool(
        name = "rectToPolar",
        description = "Rectangular (real, imag) → polar (magnitude, angleDegrees)."
    )]
    fn rect_to_polar(Parameters(p): Parameters<ComplexUnaryParams>) -> String {
        complex::rect_to_polar(&p.z)
    }

    #[tool(
        name = "complexSqrt",
        description = "Principal square root of a complex number."
    )]
    fn complex_sqrt(Parameters(p): Parameters<ComplexUnaryParams>) -> String {
        complex::complex_sqrt(&p.z)
    }

    // ---- Crypto / encoding -------------------------------------------- //

    #[tool(
        name = "hashMd5",
        description = "MD5 digest of UTF-8 input. Hex-encoded."
    )]
    fn hash_md5(Parameters(p): Parameters<StringInputParams>) -> String {
        crypto::hash_md5(&p.input)
    }

    #[tool(
        name = "hashSha1",
        description = "SHA-1 digest of UTF-8 input. Hex-encoded."
    )]
    fn hash_sha1(Parameters(p): Parameters<StringInputParams>) -> String {
        crypto::hash_sha1(&p.input)
    }

    #[tool(
        name = "hashSha256",
        description = "SHA-256 digest of UTF-8 input. Hex-encoded."
    )]
    fn hash_sha256(Parameters(p): Parameters<StringInputParams>) -> String {
        crypto::hash_sha256(&p.input)
    }

    #[tool(
        name = "hashSha512",
        description = "SHA-512 digest of UTF-8 input. Hex-encoded."
    )]
    fn hash_sha512(Parameters(p): Parameters<StringInputParams>) -> String {
        crypto::hash_sha512(&p.input)
    }

    #[tool(
        name = "base64Encode",
        description = "Base64 (standard alphabet) encode of UTF-8 input."
    )]
    fn base64_encode(Parameters(p): Parameters<StringInputParams>) -> String {
        crypto::base64_encode(&p.input)
    }

    #[tool(
        name = "base64Decode",
        description = "Base64 decode → UTF-8 string. Errors on invalid base64 or non-UTF-8 bytes."
    )]
    fn base64_decode(Parameters(p): Parameters<StringInputParams>) -> String {
        crypto::base64_decode(&p.input)
    }

    #[tool(
        name = "urlEncode",
        description = "Percent-encode a UTF-8 string for safe URL use."
    )]
    fn url_encode(Parameters(p): Parameters<StringInputParams>) -> String {
        crypto::url_encode(&p.input)
    }

    #[tool(
        name = "urlDecode",
        description = "Percent-decode a URL-encoded string back to UTF-8."
    )]
    fn url_decode(Parameters(p): Parameters<StringInputParams>) -> String {
        crypto::url_decode(&p.input)
    }

    #[tool(
        name = "hexEncode",
        description = "Lowercase hex encode of UTF-8 input bytes."
    )]
    fn hex_encode(Parameters(p): Parameters<StringInputParams>) -> String {
        crypto::hex_encode(&p.input)
    }

    #[tool(
        name = "crc32",
        description = "CRC-32 (IEEE) of UTF-8 input. Returns DECIMAL and HEX."
    )]
    fn crc32(Parameters(p): Parameters<StringInputParams>) -> String {
        crypto::crc32(&p.input)
    }

    // ---- Matrices ------------------------------------------------------ //

    #[tool(
        name = "matrixAdd",
        description = "Element-wise addition of two same-shape matrices."
    )]
    fn matrix_add(Parameters(p): Parameters<MatrixBinaryParams>) -> String {
        matrices::matrix_add(&p.a, &p.b)
    }

    #[tool(
        name = "matrixMultiply",
        description = "Standard matrix multiplication (a.cols == b.rows)."
    )]
    fn matrix_multiply(Parameters(p): Parameters<MatrixBinaryParams>) -> String {
        matrices::matrix_mult(&p.a, &p.b)
    }

    #[tool(
        name = "matrixTranspose",
        description = "Transpose a matrix (rows ↔ columns)."
    )]
    fn matrix_transpose(Parameters(p): Parameters<MatrixParams>) -> String {
        matrices::matrix_transpose(&p.a)
    }

    #[tool(
        name = "matrixDeterminant",
        description = "Determinant via partial-pivoted Gaussian elimination."
    )]
    fn matrix_determinant(Parameters(p): Parameters<MatrixParams>) -> String {
        matrices::matrix_determinant(&p.a)
    }

    #[tool(
        name = "matrixInverse",
        description = "Inverse via Gauss-Jordan elimination (singular matrices error)."
    )]
    fn matrix_inverse(Parameters(p): Parameters<MatrixParams>) -> String {
        matrices::matrix_inverse(&p.a)
    }

    #[tool(
        name = "matrixTrace",
        description = "Trace = sum of diagonal entries (square matrices only)."
    )]
    fn matrix_trace(Parameters(p): Parameters<MatrixParams>) -> String {
        matrices::matrix_trace(&p.a)
    }

    #[tool(
        name = "matrixRank",
        description = "Rank via Gauss-Jordan elimination with EPS=1e-9 pivot threshold."
    )]
    fn matrix_rank(Parameters(p): Parameters<MatrixParams>) -> String {
        matrices::matrix_rank(&p.a)
    }

    #[tool(
        name = "matrixEigenvalues2x2",
        description = "Eigenvalues of a 2x2 matrix (real or complex conjugate pair)."
    )]
    fn matrix_eigenvalues_2x2(Parameters(p): Parameters<MatrixParams>) -> String {
        matrices::matrix_eigenvalues_2x2(&p.a)
    }

    #[tool(
        name = "crossProduct",
        description = "3D cross product a × b. Vectors are \"x,y,z\" CSV."
    )]
    fn cross_product(Parameters(p): Parameters<MatrixBinaryParams>) -> String {
        matrices::cross_product(&p.a, &p.b)
    }

    #[tool(
        name = "gaussianElimination",
        description = "Solve Ax = b via partial-pivoted Gaussian elimination on the augmented matrix."
    )]
    fn gaussian_elimination(Parameters(p): Parameters<GaussianEliminationParams>) -> String {
        matrices::gaussian_elimination(&p.coefficients)
    }

    // ---- Physics ------------------------------------------------------- //

    #[tool(
        name = "kinematics",
        description = "Constant-acceleration kinematics: returns final velocity and displacement."
    )]
    fn kinematics(Parameters(p): Parameters<KinematicsParams>) -> String {
        physics::kinematics(&p.initial_velocity, &p.acceleration, &p.time)
    }

    #[tool(
        name = "projectileMotion",
        description = "Projectile motion (no air resistance): range, peak height, time of flight."
    )]
    fn projectile_motion(Parameters(p): Parameters<ProjectileParams>) -> String {
        physics::projectile_motion(&p.speed, &p.angle_degrees, &p.gravity)
    }

    #[tool(name = "newtonsForce", description = "Newton's second law: F = m·a.")]
    fn newtons_force(Parameters(p): Parameters<MassAccelParams>) -> String {
        physics::newtons_force(&p.mass, &p.acceleration)
    }

    #[tool(
        name = "gravitationalForce",
        description = "Universal gravitation: F = G·m1·m2 / r²."
    )]
    fn gravitational_force(Parameters(p): Parameters<GravitationalForceParams>) -> String {
        physics::gravitational_force(&p.m1, &p.m2, &p.distance)
    }

    #[tool(
        name = "dopplerEffect",
        description = "Classical Doppler shift for sound. Approaching → positive velocity."
    )]
    fn doppler_effect(Parameters(p): Parameters<DopplerParams>) -> String {
        physics::doppler_effect(
            &p.source_freq,
            &p.sound_speed,
            &p.source_velocity,
            &p.observer_velocity,
        )
    }

    #[tool(name = "waveLength", description = "λ = waveSpeed / frequency.")]
    fn wave_length(Parameters(p): Parameters<WaveLengthParams>) -> String {
        physics::wave_length(&p.frequency, &p.wave_speed)
    }

    #[tool(name = "planckEnergy", description = "Photon energy E = h·f (joules).")]
    fn planck_energy(Parameters(p): Parameters<FrequencyParams>) -> String {
        physics::planck_energy(&p.frequency)
    }

    #[tool(
        name = "idealGasLaw",
        description = "PV = nRT solver. solveFor=P|V|n|T (provide the other three)."
    )]
    fn ideal_gas_law(Parameters(p): Parameters<IdealGasLawParams>) -> String {
        physics::ideal_gas_law(
            &p.pressure,
            &p.volume,
            &p.moles,
            &p.temperature,
            &p.solve_for,
        )
    }

    #[tool(
        name = "heatTransfer",
        description = "Conduction Q = k·A·ΔT / thickness (Fourier's law)."
    )]
    fn heat_transfer(Parameters(p): Parameters<HeatTransferParams>) -> String {
        physics::heat_transfer(
            &p.thermal_conductivity,
            &p.area,
            &p.delta_temp,
            &p.thickness,
        )
    }

    #[tool(
        name = "stefanBoltzmann",
        description = "Radiated power P = σ·ε·A·T⁴ (T in Kelvin, ε in [0,1])."
    )]
    fn stefan_boltzmann(Parameters(p): Parameters<StefanBoltzmannParams>) -> String {
        physics::stefan_boltzmann(&p.emissivity, &p.area, &p.temperature_k)
    }

    #[tool(name = "escapeVelocity", description = "Escape velocity v = √(2GM/r).")]
    fn escape_velocity(Parameters(p): Parameters<MassRadiusParams>) -> String {
        physics::escape_velocity(&p.mass, &p.radius)
    }

    #[tool(
        name = "orbitalVelocity",
        description = "Circular orbital velocity v = √(GM/r)."
    )]
    fn orbital_velocity(Parameters(p): Parameters<MassRadiusParams>) -> String {
        physics::orbital_velocity(&p.mass, &p.radius)
    }

    // ---- Chemistry ----------------------------------------------------- //

    #[tool(
        name = "molarMass",
        description = "Molar mass of a chemical formula. Supports nested parens like Fe2(SO4)3."
    )]
    fn molar_mass(Parameters(p): Parameters<FormulaParams>) -> String {
        chemistry::molar_mass(&p.formula)
    }

    #[tool(name = "ph", description = "pH from [H⁺] (mol/L): pH = -log10([H⁺]).")]
    fn ph(Parameters(p): Parameters<PhParams>) -> String {
        chemistry::ph(&p.h_concentration)
    }

    #[tool(
        name = "poh",
        description = "pOH from [OH⁻] (mol/L): pOH = -log10([OH⁻])."
    )]
    fn poh(Parameters(p): Parameters<PohParams>) -> String {
        chemistry::poh(&p.oh_concentration)
    }

    #[tool(
        name = "molarity",
        description = "Molarity (mol/L) = moles / volumeLitres."
    )]
    fn molarity(Parameters(p): Parameters<MolarityParams>) -> String {
        chemistry::molarity(&p.moles, &p.volume_litres)
    }

    #[tool(
        name = "molality",
        description = "Molality (mol/kg) = moles / kilogramsSolvent."
    )]
    fn molality(Parameters(p): Parameters<MolalityParams>) -> String {
        chemistry::molality(&p.moles, &p.kilograms_solvent)
    }

    #[tool(
        name = "hendersonHasselbalch",
        description = "pH = pKa + log10([conjugateBase] / [weakAcid])."
    )]
    fn henderson_hasselbalch(Parameters(p): Parameters<HendersonHasselbalchParams>) -> String {
        chemistry::henderson_hasselbalch(&p.pka, &p.conjugate_base, &p.weak_acid)
    }

    #[tool(name = "halfLife", description = "Half-life t½ = ln(2) / λ.")]
    fn half_life(Parameters(p): Parameters<DecayParams>) -> String {
        chemistry::half_life(&p.decay_constant)
    }

    #[tool(name = "decayConstant", description = "Decay constant λ = ln(2) / t½.")]
    fn decay_constant(Parameters(p): Parameters<HalfLifeInputParams>) -> String {
        chemistry::decay_constant(&p.half_life)
    }

    #[tool(
        name = "idealGasMoles",
        description = "Moles of an ideal gas: n = PV / (RT). SI units (Pa, m³, K)."
    )]
    fn ideal_gas_moles(Parameters(p): Parameters<IdealGasMolesParams>) -> String {
        chemistry::ideal_gas_moles(&p.pressure_pa, &p.volume_m3, &p.temperature_k)
    }
}

#[tool_handler]
impl ServerHandler for MathCalcServer {
    fn get_info(&self) -> ServerInfo {
        // Pull the live tool count from the registered router so the
        // instructions never drift from the actual surface when tools are
        // added or removed. Using `self.tool_router` here also gives this
        // method a genuine reason to take `&self`.
        let tool_count = self.tool_router.list_all().len();
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::LATEST)
            .with_server_info(Implementation::from_build_env())
            .with_instructions(format!(
                "Pure-Rust math calculator MCP server. {tool_count} tools across 23 categories. \
                 Response format: `TOOL_NAME: OK | KEY: value | ...` (inline) or block layout with `ROW_N` keys for tabular payloads. \
                 Errors: `TOOL_NAME: ERROR` + `REASON: [CODE] text` + optional `DETAIL: k=v`. \
                 Error codes: DOMAIN_ERROR, OUT_OF_RANGE, DIVISION_BY_ZERO, PARSE_ERROR, INVALID_INPUT, UNKNOWN_VARIABLE, UNKNOWN_FUNCTION, OVERFLOW, NOT_IMPLEMENTED. \
                 Categories: \
                 Basic (add, subtract, multiply, divide, power, modulo, abs); \
                 Scientific (sqrt, log, log10, factorial, sin, cos, tan); \
                 Programmable (evaluate, evaluateWithVariables, evaluateExact, evaluateExactWithVariables) — supports constants pi/e/tau/phi and functions exp, ln, log2, asin/acos/atan/atan2, sinh/cosh/tanh, asinh/acosh/atanh, cbrt, round, trunc, sign, factorial, min/max/mod/hypot/pow/gcd/lcm; \
                 Vector/SIMD (sumArray, dotProduct, scaleArray, magnitudeArray); \
                 Financial (compoundInterest, loanPayment, presentValue, futureValueAnnuity, returnOnInvestment, amortizationSchedule); \
                 Calculus (derivative, nthDerivative, definiteIntegral, tangentLine); \
                 Unit converter (convert, convertAutoDetect); \
                 Cooking (convertCookingVolume, convertCookingWeight, convertOvenTemperature); \
                 Measure reference (listCategories, listToolCategories, listUnits, getConversionFactor, explainConversion); \
                 DateTime (convertTimezone, formatDateTime, currentDateTime, listTimezones, dateTimeDifference); \
                 Printing tape (calculateWithTape); \
                 Graphing (plotFunction, solveEquation, findRoots); \
                 Network (subnetCalculator, ipToBinary, binaryToIp, ipToDecimal, decimalToIp, ipInSubnet, vlsmSubnets, summarizeSubnets, expandIpv6, compressIpv6, transferTime, throughput, tcpThroughput); \
                 Analog electronics (ohmsLaw, resistorCombination, capacitorCombination, inductorCombination, voltageDivider, currentDivider, rcTimeConstant, rlTimeConstant, rlcResonance, impedance, decibelConvert, filterCutoff, ledResistor, wheatstoneBridge); \
                 Digital electronics (convertBase, twosComplement, grayCode, bitwiseOp, adcResolution, dacOutput, timer555Astable, timer555Monostable, frequencyPeriod, nyquistRate); \
                 Statistics (mean, median, mode, variance, stdDev, percentile, quartile, iqr, correlation, covariance, linearRegression, normalPdf, normalCdf, tTestOneSample, binomialPmf, confidenceInterval); \
                 Combinatorics (combination, permutation, fibonacci, isPrime, nextPrime, primeFactors, eulerTotient); \
                 Geometry (circleArea, circlePerimeter, sphereVolume, sphereArea, triangleArea, polygonArea, coneVolume, cylinderVolume, distance2D, distance3D, regularPolygon, pointToLineDistance); \
                 Complex numbers (complexAdd, complexMult, complexDiv, complexConjugate, complexPower, complexMagnitude, complexPhase, polarToRect, rectToPolar, complexSqrt); \
                 Crypto/Encoding (hashMd5, hashSha1, hashSha256, hashSha512, base64Encode, base64Decode, urlEncode, urlDecode, hexEncode, crc32); \
                 Matrices (matrixAdd, matrixMultiply, matrixTranspose, matrixDeterminant, matrixInverse, matrixTrace, matrixRank, matrixEigenvalues2x2, crossProduct, gaussianElimination); \
                 Physics (kinematics, projectileMotion, newtonsForce, gravitationalForce, dopplerEffect, waveLength, planckEnergy, idealGasLaw, heatTransfer, stefanBoltzmann, escapeVelocity, orbitalVelocity); \
                 Chemistry (molarMass, ph, poh, molarity, molality, hendersonHasselbalch, halfLife, decayConstant, idealGasMoles).",
            ))
    }
}
