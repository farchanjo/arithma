//! Unit conversion registry — 1:1 port of Java
//! `com.archanjo.mathcalculator.engine.UnitRegistry` +
//! `UnitCategory` + `UnitDefinition`.
//!
//! Arithmetic matches Java `MathContext.DECIMAL128` (34 significant digits)
//! combined with `RoundingMode.HALF_UP`:
//!
//! * multiplications use [`Context`] with precision 34 + HALF_UP,
//! * divisions use [`BigDecimal::with_scale_round`] at scale 34 + HALF_UP,
//!
//! matching `BigDecimal.divide(divisor, FACTOR_SCALE, ROUNDING)` semantics.
//!
//! Linear conversions apply `value * from.to_base_factor / to.to_base_factor`.
//! Temperature conversions use formula-based routing through Celsius.
//! Gas mark uses a fixed lookup table.

use std::collections::HashMap;
use std::sync::LazyLock;

use bigdecimal::{BigDecimal, Context, RoundingMode};
use num_traits::One;

use crate::engine::bigdecimal_ext::{DECIMAL128_PRECISION, strip_plain};

// ------------------------------------------------------------------ //
//  Errors
// ------------------------------------------------------------------ //

/// Errors surfaced by the unit registry.
///
/// Messages match the Java `IllegalArgumentException` strings verbatim so
/// downstream MCP clients observe identical text.
#[derive(Debug, thiserror::Error)]
pub enum UnitError {
    #[error("Unknown unit: {0}")]
    UnknownUnit(String),

    #[error("Cannot convert between {from} ({from_cat}) and {to} ({to_cat})")]
    CrossCategory {
        from: String,
        from_cat: String,
        to: String,
        to_cat: String,
    },

    #[error("Temperature uses formulas, not a fixed factor")]
    TemperatureFactor,

    #[error("Unknown temperature unit: {0}")]
    UnknownTemperatureUnit(String),

    #[error("Gas mark must be 1-10. Received: {0}")]
    InvalidGasMark(i32),

    #[error("Unit '{code}' is not in category {category}")]
    WrongCategory { code: String, category: String },

    #[error("Unknown category: {0}")]
    UnknownCategory(String),
}

// ------------------------------------------------------------------ //
//  UnitCategory — mirrors the Java enum in declaration order
// ------------------------------------------------------------------ //

/// Categories of measurable physical quantities supported by the converter.
///
/// Variants are declared in the same order as the Java enum so
/// [`UnitCategory::all`] produces identical ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnitCategory {
    DataStorage,
    Length,
    Mass,
    Volume,
    Temperature,
    Time,
    Speed,
    Area,
    Energy,
    Force,
    Pressure,
    Power,
    Density,
    Frequency,
    Angle,
    DataRate,
    Resistance,
    Capacitance,
    Inductance,
    Voltage,
    Current,
}

const ALL_CATEGORIES: [UnitCategory; 21] = [
    UnitCategory::DataStorage,
    UnitCategory::Length,
    UnitCategory::Mass,
    UnitCategory::Volume,
    UnitCategory::Temperature,
    UnitCategory::Time,
    UnitCategory::Speed,
    UnitCategory::Area,
    UnitCategory::Energy,
    UnitCategory::Force,
    UnitCategory::Pressure,
    UnitCategory::Power,
    UnitCategory::Density,
    UnitCategory::Frequency,
    UnitCategory::Angle,
    UnitCategory::DataRate,
    UnitCategory::Resistance,
    UnitCategory::Capacitance,
    UnitCategory::Inductance,
    UnitCategory::Voltage,
    UnitCategory::Current,
];

impl UnitCategory {
    /// Uppercase name matching the Java enum literal (e.g. `"LENGTH"`).
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DataStorage => "DATA_STORAGE",
            Self::Length => "LENGTH",
            Self::Mass => "MASS",
            Self::Volume => "VOLUME",
            Self::Temperature => "TEMPERATURE",
            Self::Time => "TIME",
            Self::Speed => "SPEED",
            Self::Area => "AREA",
            Self::Energy => "ENERGY",
            Self::Force => "FORCE",
            Self::Pressure => "PRESSURE",
            Self::Power => "POWER",
            Self::Density => "DENSITY",
            Self::Frequency => "FREQUENCY",
            Self::Angle => "ANGLE",
            Self::DataRate => "DATA_RATE",
            Self::Resistance => "RESISTANCE",
            Self::Capacitance => "CAPACITANCE",
            Self::Inductance => "INDUCTANCE",
            Self::Voltage => "VOLTAGE",
            Self::Current => "CURRENT",
        }
    }

    /// Parse the uppercase Java name back into a category (case-insensitive).
    ///
    /// # Errors
    /// Returns [`UnitError::UnknownCategory`] when the text doesn't match any
    /// declared variant.
    pub fn parse(s: &str) -> Result<Self, UnitError> {
        let upper = s.to_ascii_uppercase();
        for cat in ALL_CATEGORIES {
            if cat.as_str() == upper {
                return Ok(cat);
            }
        }
        Err(UnitError::UnknownCategory(s.to_string()))
    }

    /// All 21 categories in Java declaration order.
    #[must_use]
    pub fn all() -> &'static [UnitCategory] {
        &ALL_CATEGORIES
    }
}

impl std::fmt::Display for UnitCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ------------------------------------------------------------------ //
//  UnitDefinition
// ------------------------------------------------------------------ //

/// A unit of measurement with its conversion factor to the category's base.
///
/// `to_base_factor` is [`None`] for temperature units (which use formulas).
#[derive(Debug, Clone)]
pub struct UnitDefinition {
    pub code: String,
    pub name: String,
    pub category: UnitCategory,
    pub to_base_factor: Option<BigDecimal>,
}

// ------------------------------------------------------------------ //
//  Arithmetic helpers — DECIMAL128 precision + HALF_UP rounding
// ------------------------------------------------------------------ //

/// Scale applied to every division — matches Java `FACTOR_SCALE = 34`.
const FACTOR_SCALE: i64 = 34;

/// DECIMAL128 context: 34 digits of precision, `HALF_UP` rounding.
fn decimal128_context() -> Context {
    Context::default()
        .with_prec(DECIMAL128_PRECISION)
        .expect("DECIMAL128_PRECISION is non-zero")
        .with_rounding_mode(RoundingMode::HalfUp)
}

/// Multiplication mirroring `a.multiply(b, PRECISION)` in Java.
fn mul(a: &BigDecimal, b: &BigDecimal) -> BigDecimal {
    decimal128_context().multiply(a, b)
}

/// Division mirroring `a.divide(b, FACTOR_SCALE, ROUNDING)` in Java:
/// quotient truncated/rounded to scale 34 with HALF_UP.
fn div_scale(a: &BigDecimal, b: &BigDecimal) -> BigDecimal {
    let quotient = a / b;
    quotient.with_scale_round(FACTOR_SCALE, RoundingMode::HalfUp)
}

/// Addition with DECIMAL128 precision (matches Java `.add(b, PRECISION)`).
fn add(a: &BigDecimal, b: &BigDecimal) -> BigDecimal {
    let ctx = decimal128_context();
    (a + b).with_precision_round(
        std::num::NonZeroU64::new(DECIMAL128_PRECISION).expect("non-zero"),
        ctx.rounding_mode(),
    )
}

/// Subtraction with DECIMAL128 precision (matches Java `.subtract(b, PRECISION)`).
fn sub(a: &BigDecimal, b: &BigDecimal) -> BigDecimal {
    let diff = a - b;
    diff.with_precision_round(
        std::num::NonZeroU64::new(DECIMAL128_PRECISION).expect("non-zero"),
        RoundingMode::HalfUp,
    )
}

/// `base^exp` rounded to DECIMAL128 precision.
fn pow_ctx(base: &BigDecimal, exp: i64) -> BigDecimal {
    base.powi_with_context(exp, &decimal128_context())
}

// ------------------------------------------------------------------ //
//  Literal constants — pre-parsed BigDecimals
// ------------------------------------------------------------------ //

fn bd(literal: &str) -> BigDecimal {
    literal.parse().expect("valid BigDecimal literal")
}

static ONE: LazyLock<BigDecimal> = LazyLock::new(BigDecimal::one);

static THOUSAND: LazyLock<BigDecimal> = LazyLock::new(|| bd("1000"));
static SECONDS_PER_HOUR: LazyLock<BigDecimal> = LazyLock::new(|| bd("3600"));
static MILLI: LazyLock<BigDecimal> = LazyLock::new(|| bd("0.001"));
static SIXTY: LazyLock<BigDecimal> = LazyLock::new(|| bd("60"));

// Exact SI building blocks
static POUND_KG: LazyLock<BigDecimal> = LazyLock::new(|| bd("0.45359237"));
static GRAVITY: LazyLock<BigDecimal> = LazyLock::new(|| bd("9.80665"));
static INCH_M: LazyLock<BigDecimal> = LazyLock::new(|| bd("0.0254"));
static FOOT_M: LazyLock<BigDecimal> = LazyLock::new(|| bd("0.3048"));

/// `LBF_N = POUND_KG * GRAVITY` (pound-force in newtons).
static LBF_N: LazyLock<BigDecimal> = LazyLock::new(|| mul(&POUND_KG, &GRAVITY));

static PI_VALUE: LazyLock<BigDecimal> =
    LazyLock::new(|| bd("3.1415926535897932384626433832795028841972"));

// Derived factors (computed once at first access)
static PSI_PA: LazyLock<BigDecimal> = LazyLock::new(|| div_scale(&LBF_N, &pow_ctx(&INCH_M, 2)));
static TORR_PA: LazyLock<BigDecimal> = LazyLock::new(|| div_scale(&bd("101325"), &bd("760")));
static HP_W: LazyLock<BigDecimal> = LazyLock::new(|| mul(&mul(&bd("550"), &FOOT_M), &LBF_N));
static KMH_MS: LazyLock<BigDecimal> = LazyLock::new(|| div_scale(&THOUSAND, &SECONDS_PER_HOUR));
static KNOT_MS: LazyLock<BigDecimal> = LazyLock::new(|| div_scale(&bd("1852"), &SECONDS_PER_HOUR));
static DEG_PER_RAD: LazyLock<BigDecimal> = LazyLock::new(|| div_scale(&bd("180"), &PI_VALUE));
static RPM_HZ: LazyLock<BigDecimal> = LazyLock::new(|| div_scale(&ONE, &SIXTY));
static BTU_H_W: LazyLock<BigDecimal> =
    LazyLock::new(|| div_scale(&bd("1055.05585262"), &SECONDS_PER_HOUR));
static ARCMIN_DEG: LazyLock<BigDecimal> = LazyLock::new(|| div_scale(&ONE, &SIXTY));
static ARCSEC_DEG: LazyLock<BigDecimal> = LazyLock::new(|| div_scale(&ONE, &SECONDS_PER_HOUR));

// Electrical / data-rate prefix constants
static MILLION: LazyLock<BigDecimal> = LazyLock::new(|| bd("1000000"));
static BILLION: LazyLock<BigDecimal> = LazyLock::new(|| bd("1000000000"));
static TRILLION: LazyLock<BigDecimal> = LazyLock::new(|| bd("1000000000000"));
static EIGHT: LazyLock<BigDecimal> = LazyLock::new(|| bd("8"));
static MICRO: LazyLock<BigDecimal> = LazyLock::new(|| bd("0.000001"));
static NANO: LazyLock<BigDecimal> = LazyLock::new(|| bd("0.000000001"));
static PICO: LazyLock<BigDecimal> = LazyLock::new(|| bd("0.000000000001"));

// Temperature constants
static NINE: LazyLock<BigDecimal> = LazyLock::new(|| bd("9"));
static FIVE: LazyLock<BigDecimal> = LazyLock::new(|| bd("5"));
static THIRTY_TWO: LazyLock<BigDecimal> = LazyLock::new(|| bd("32"));
static KELVIN_OFFSET: LazyLock<BigDecimal> = LazyLock::new(|| bd("273.15"));
static RANKINE_OFFSET: LazyLock<BigDecimal> = LazyLock::new(|| bd("491.67"));
static RANKINE_RATIO: LazyLock<BigDecimal> = LazyLock::new(|| div_scale(&FIVE, &NINE));

// ------------------------------------------------------------------ //
//  Registry storage — populated once via LazyLock
// ------------------------------------------------------------------ //

/// Ordered list of unit codes, matching the Java `LinkedHashMap` iteration order.
static UNIT_ORDER: LazyLock<Vec<&'static str>> = LazyLock::new(build_unit_order);

/// All unit definitions keyed by lowercase code.
static UNITS: LazyLock<HashMap<&'static str, UnitDefinition>> = LazyLock::new(build_units);

/// Flat ordered vector mirroring [`UNIT_ORDER`], returned by [`all_units`].
static UNITS_FLAT: LazyLock<Vec<&'static UnitDefinition>> = LazyLock::new(|| {
    UNIT_ORDER
        .iter()
        .map(|code| UNITS.get(code).expect("registered unit"))
        .collect()
});

/// Category → ordered list of unit codes.
static CATEGORY_INDEX: LazyLock<HashMap<UnitCategory, Vec<&'static str>>> =
    LazyLock::new(build_category_index);

/// Gas mark (1–10) → Celsius temperature.
static GAS_MARK_TO_C: LazyLock<Vec<(i32, BigDecimal)>> = LazyLock::new(|| {
    vec![
        (1, bd("140")),
        (2, bd("150")),
        (3, bd("170")),
        (4, bd("180")),
        (5, bd("190")),
        (6, bd("200")),
        (7, bd("220")),
        (8, bd("230")),
        (9, bd("240")),
        (10, bd("260")),
    ]
});

/// Temperature pairwise conversion phrase lookup.
static TEMP_FORMULAS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut m = HashMap::with_capacity(12);
    m.insert("c->f", "F = C * 9/5 + 32");
    m.insert("f->c", "C = (F - 32) * 5/9");
    m.insert("c->k", "K = C + 273.15");
    m.insert("k->c", "C = K - 273.15");
    m.insert("c->r", "R = C * 9/5 + 491.67");
    m.insert("r->c", "C = (R - 491.67) * 5/9");
    m.insert("f->k", "K = (F - 32) * 5/9 + 273.15");
    m.insert("k->f", "F = (K - 273.15) * 9/5 + 32");
    m.insert("f->r", "R = F + 459.67");
    m.insert("r->f", "F = R - 459.67");
    m.insert("k->r", "R = K * 9/5");
    m.insert("r->k", "K = R * 5/9");
    m
});

// ------------------------------------------------------------------ //
//  Registry construction
// ------------------------------------------------------------------ //

struct Reg {
    order: Vec<&'static str>,
    units: HashMap<&'static str, UnitDefinition>,
    index: HashMap<UnitCategory, Vec<&'static str>>,
}

impl Reg {
    fn new() -> Self {
        Self {
            order: Vec::with_capacity(128),
            units: HashMap::with_capacity(128),
            index: HashMap::with_capacity(ALL_CATEGORIES.len()),
        }
    }

    fn reg(
        &mut self,
        code: &'static str,
        name: &'static str,
        cat: UnitCategory,
        factor: Option<BigDecimal>,
    ) {
        let def = UnitDefinition {
            code: code.to_string(),
            name: name.to_string(),
            category: cat,
            to_base_factor: factor,
        };
        self.order.push(code);
        self.units.insert(code, def);
        self.index.entry(cat).or_default().push(code);
    }

    fn reg_factor(
        &mut self,
        code: &'static str,
        name: &'static str,
        cat: UnitCategory,
        factor: BigDecimal,
    ) {
        self.reg(code, name, cat, Some(factor));
    }

    fn reg_base(&mut self, code: &'static str, name: &'static str, cat: UnitCategory) {
        self.reg(code, name, cat, Some(BigDecimal::one()));
    }

    fn reg_temp(&mut self, code: &'static str, name: &'static str) {
        self.reg(code, name, UnitCategory::Temperature, None);
    }
}

fn populate(reg: &mut Reg) {
    register_data_storage(reg);
    register_length(reg);
    register_mass(reg);
    register_volume(reg);
    register_temperature(reg);
    register_time(reg);
    register_speed(reg);
    register_area(reg);
    register_energy(reg);
    register_force(reg);
    register_pressure(reg);
    register_power(reg);
    register_density(reg);
    register_frequency(reg);
    register_angle(reg);
    register_data_rate(reg);
    register_resistance(reg);
    register_capacitance(reg);
    register_inductance(reg);
    register_voltage(reg);
    register_current(reg);
}

fn build_registry() -> Reg {
    let mut reg = Reg::new();
    populate(&mut reg);
    reg
}

fn build_unit_order() -> Vec<&'static str> {
    build_registry().order
}

fn build_units() -> HashMap<&'static str, UnitDefinition> {
    build_registry().units
}

fn build_category_index() -> HashMap<UnitCategory, Vec<&'static str>> {
    build_registry().index
}

// --- per-category registrations (each < 30 lines, mirrors Java) ---

fn register_data_storage(reg: &mut Reg) {
    let cat = UnitCategory::DataStorage;
    reg.reg_base("byte", "byte", cat);
    reg.reg_factor("bit", "bit", cat, bd("0.125"));
    reg.reg_factor("kb", "kilobyte", cat, bd("1024"));
    reg.reg_factor("mb", "megabyte", cat, bd("1048576"));
    reg.reg_factor("gb", "gigabyte", cat, bd("1073741824"));
    reg.reg_factor("tb", "terabyte", cat, bd("1099511627776"));
    reg.reg_factor("pb", "petabyte", cat, bd("1125899906842624"));
}

fn register_length(reg: &mut Reg) {
    let cat = UnitCategory::Length;
    reg.reg_base("m", "meter", cat);
    reg.reg_factor("mm", "millimeter", cat, MILLI.clone());
    reg.reg_factor("cm", "centimeter", cat, bd("0.01"));
    reg.reg_factor("km", "kilometer", cat, THOUSAND.clone());
    reg.reg_factor("in", "inch", cat, INCH_M.clone());
    reg.reg_factor("ft", "foot", cat, FOOT_M.clone());
    reg.reg_factor("yd", "yard", cat, bd("0.9144"));
    reg.reg_factor("mi", "mile", cat, bd("1609.344"));
    reg.reg_factor("nmi", "nautical mile", cat, bd("1852"));
}

fn register_mass(reg: &mut Reg) {
    let cat = UnitCategory::Mass;
    reg.reg_base("kg", "kilogram", cat);
    reg.reg_factor("g", "gram", cat, MILLI.clone());
    reg.reg_factor("mg", "milligram", cat, bd("0.000001"));
    reg.reg_factor("t", "tonne", cat, THOUSAND.clone());
    reg.reg_factor("lb", "pound", cat, POUND_KG.clone());
    reg.reg_factor("oz", "ounce", cat, bd("0.028349523125"));
    reg.reg_factor("st", "stone", cat, bd("6.35029318"));
}

fn register_volume(reg: &mut Reg) {
    let cat = UnitCategory::Volume;
    reg.reg_base("l", "liter", cat);
    reg.reg_factor("ml", "milliliter", cat, MILLI.clone());
    reg.reg_factor("m3", "cubic meter", cat, THOUSAND.clone());
    reg.reg_factor("usgal", "US gallon", cat, bd("3.785411784"));
    reg.reg_factor("igal", "imperial gallon", cat, bd("4.54609"));
    reg.reg_factor("uscup", "US cup", cat, bd("0.2365882365"));
    reg.reg_factor("tbsp", "tablespoon", cat, bd("0.01478676478125"));
    reg.reg_factor("tsp", "teaspoon", cat, bd("0.00492892159375"));
    reg.reg_factor("usfloz", "US fluid ounce", cat, bd("0.0295735295625"));
}

fn register_temperature(reg: &mut Reg) {
    reg.reg_temp("c", "Celsius");
    reg.reg_temp("f", "Fahrenheit");
    reg.reg_temp("k", "Kelvin");
    reg.reg_temp("r", "Rankine");
}

fn register_time(reg: &mut Reg) {
    let cat = UnitCategory::Time;
    reg.reg_base("s", "second", cat);
    reg.reg_factor("ms", "millisecond", cat, MILLI.clone());
    reg.reg_factor("min", "minute", cat, SIXTY.clone());
    reg.reg_factor("h", "hour", cat, SECONDS_PER_HOUR.clone());
    reg.reg_factor("d", "day", cat, bd("86400"));
    reg.reg_factor("wk", "week", cat, bd("604800"));
    reg.reg_factor("yr", "year", cat, bd("31557600"));
}

fn register_speed(reg: &mut Reg) {
    let cat = UnitCategory::Speed;
    reg.reg_base("m/s", "meter per second", cat);
    reg.reg_factor("km/h", "kilometer per hour", cat, KMH_MS.clone());
    reg.reg_factor("mph", "mile per hour", cat, bd("0.44704"));
    reg.reg_factor("kn", "knot", cat, KNOT_MS.clone());
    reg.reg_factor("ft/s", "foot per second", cat, FOOT_M.clone());
}

fn register_area(reg: &mut Reg) {
    let cat = UnitCategory::Area;
    reg.reg_base("m2", "square meter", cat);
    reg.reg_factor("cm2", "square centimeter", cat, bd("0.0001"));
    reg.reg_factor("km2", "square kilometer", cat, bd("1000000"));
    reg.reg_factor("ft2", "square foot", cat, bd("0.09290304"));
    reg.reg_factor("ac", "acre", cat, bd("4046.8564224"));
    reg.reg_factor("ha", "hectare", cat, bd("10000"));
    reg.reg_factor("mi2", "square mile", cat, bd("2589988.110336"));
}

fn register_energy(reg: &mut Reg) {
    let cat = UnitCategory::Energy;
    reg.reg_base("j", "joule", cat);
    reg.reg_factor("cal", "calorie", cat, bd("4.184"));
    reg.reg_factor("kcal", "kilocalorie", cat, bd("4184"));
    reg.reg_factor("kwh", "kilowatt-hour", cat, bd("3600000"));
    reg.reg_factor("btu", "BTU", cat, bd("1055.05585262"));
    reg.reg_factor("ev", "electronvolt", cat, bd("1.602176634E-19"));
}

fn register_force(reg: &mut Reg) {
    let cat = UnitCategory::Force;
    reg.reg_base("n", "newton", cat);
    reg.reg_factor("dyn", "dyne", cat, bd("0.00001"));
    reg.reg_factor("lbf", "pound-force", cat, LBF_N.clone());
    reg.reg_factor("kgf", "kilogram-force", cat, GRAVITY.clone());
}

fn register_pressure(reg: &mut Reg) {
    let cat = UnitCategory::Pressure;
    reg.reg_base("pa", "pascal", cat);
    reg.reg_factor("bar", "bar", cat, bd("100000"));
    reg.reg_factor("atm", "atmosphere", cat, bd("101325"));
    reg.reg_factor("psi", "pound per square inch", cat, PSI_PA.clone());
    reg.reg_factor("torr", "torr", cat, TORR_PA.clone());
    reg.reg_factor("mmhg", "millimeter of mercury", cat, bd("133.322387415"));
}

fn register_power(reg: &mut Reg) {
    let cat = UnitCategory::Power;
    reg.reg_base("w", "watt", cat);
    reg.reg_factor("kw", "kilowatt", cat, THOUSAND.clone());
    reg.reg_factor("hp", "horsepower", cat, HP_W.clone());
    reg.reg_factor("btu/h", "BTU per hour", cat, BTU_H_W.clone());
}

fn register_density(reg: &mut Reg) {
    let cat = UnitCategory::Density;
    reg.reg_base("kg/m3", "kilogram per cubic meter", cat);
    reg.reg_factor("g/cm3", "gram per cubic centimeter", cat, THOUSAND.clone());
    reg.reg_factor("g/ml", "gram per milliliter", cat, THOUSAND.clone());
    reg.reg_factor("lb/ft3", "pound per cubic foot", cat, bd("16.018463374"));
}

fn register_frequency(reg: &mut Reg) {
    let cat = UnitCategory::Frequency;
    reg.reg_base("hz", "hertz", cat);
    reg.reg_factor("khz", "kilohertz", cat, THOUSAND.clone());
    reg.reg_factor("mhz", "megahertz", cat, bd("1000000"));
    reg.reg_factor("ghz", "gigahertz", cat, bd("1000000000"));
    reg.reg_factor("rpm", "revolutions per minute", cat, RPM_HZ.clone());
}

fn register_angle(reg: &mut Reg) {
    let cat = UnitCategory::Angle;
    reg.reg_base("deg", "degree", cat);
    reg.reg_factor("rad", "radian", cat, DEG_PER_RAD.clone());
    reg.reg_factor("grad", "gradian", cat, bd("0.9"));
    reg.reg_factor("arcmin", "arcminute", cat, ARCMIN_DEG.clone());
    reg.reg_factor("arcsec", "arcsecond", cat, ARCSEC_DEG.clone());
    reg.reg_factor("turn", "turn", cat, bd("360"));
}

fn register_data_rate(reg: &mut Reg) {
    let cat = UnitCategory::DataRate;
    reg.reg_base("bps", "bit per second", cat);
    reg.reg_factor("kbps", "kilobit per second", cat, THOUSAND.clone());
    reg.reg_factor("mbps", "megabit per second", cat, MILLION.clone());
    reg.reg_factor("gbps", "gigabit per second", cat, BILLION.clone());
    reg.reg_factor("tbps", "terabit per second", cat, TRILLION.clone());
    reg.reg_factor("byps", "byte per second", cat, EIGHT.clone());
    reg.reg_factor("kbyps", "kilobyte per second", cat, bd("8000"));
    reg.reg_factor("mbyps", "megabyte per second", cat, bd("8000000"));
    reg.reg_factor("gbyps", "gigabyte per second", cat, bd("8000000000"));
}

fn register_resistance(reg: &mut Reg) {
    let cat = UnitCategory::Resistance;
    reg.reg_base("ohm", "ohm", cat);
    reg.reg_factor("mohm", "milliohm", cat, MILLI.clone());
    reg.reg_factor("kohm", "kiloohm", cat, THOUSAND.clone());
    reg.reg_factor("megohm", "megaohm", cat, MILLION.clone());
}

fn register_capacitance(reg: &mut Reg) {
    let cat = UnitCategory::Capacitance;
    reg.reg_base("fd", "farad", cat);
    reg.reg_factor("mfd", "millifarad", cat, MILLI.clone());
    reg.reg_factor("uf", "microfarad", cat, MICRO.clone());
    reg.reg_factor("nf", "nanofarad", cat, NANO.clone());
    reg.reg_factor("pf", "picofarad", cat, PICO.clone());
}

fn register_inductance(reg: &mut Reg) {
    let cat = UnitCategory::Inductance;
    reg.reg_base("hy", "henry", cat);
    reg.reg_factor("mhy", "millihenry", cat, MILLI.clone());
    reg.reg_factor("uhy", "microhenry", cat, MICRO.clone());
    reg.reg_factor("nhy", "nanohenry", cat, NANO.clone());
}

fn register_voltage(reg: &mut Reg) {
    let cat = UnitCategory::Voltage;
    reg.reg_base("vlt", "volt", cat);
    reg.reg_factor("mvlt", "millivolt", cat, MILLI.clone());
    reg.reg_factor("kvlt", "kilovolt", cat, THOUSAND.clone());
    reg.reg_factor("uvlt", "microvolt", cat, MICRO.clone());
}

fn register_current(reg: &mut Reg) {
    let cat = UnitCategory::Current;
    reg.reg_base("amp", "ampere", cat);
    reg.reg_factor("mamp", "milliampere", cat, MILLI.clone());
    reg.reg_factor("uamp", "microampere", cat, MICRO.clone());
    reg.reg_factor("namp", "nanoampere", cat, NANO.clone());
}

// ------------------------------------------------------------------ //
//  Lookup helpers
// ------------------------------------------------------------------ //

fn normalize(code: &str) -> String {
    code.to_ascii_lowercase()
}

fn require_unit(code: &str) -> Result<&'static UnitDefinition, UnitError> {
    let key = normalize(code);
    UNITS
        .get(key.as_str())
        .ok_or_else(|| UnitError::UnknownUnit(code.to_string()))
}

fn require_same_category(
    source: &UnitDefinition,
    target: &UnitDefinition,
) -> Result<(), UnitError> {
    if source.category == target.category {
        Ok(())
    } else {
        Err(UnitError::CrossCategory {
            from: source.code.clone(),
            from_cat: source.category.as_str().to_string(),
            to: target.code.clone(),
            to_cat: target.category.as_str().to_string(),
        })
    }
}

// ------------------------------------------------------------------ //
//  Public API
// ------------------------------------------------------------------ //

/// Convert `value` from unit `from` to unit `to`.
///
/// # Errors
/// * [`UnitError::UnknownUnit`] if either code is not registered.
/// * [`UnitError::CrossCategory`] if the two units belong to different
///   categories.
/// * [`UnitError::UnknownTemperatureUnit`] if a temperature code is malformed
///   (should not happen for registered codes).
pub fn convert(value: &BigDecimal, from: &str, to: &str) -> Result<BigDecimal, UnitError> {
    let source = require_unit(from)?;
    let target = require_unit(to)?;
    require_same_category(source, target)?;

    if source.category == UnitCategory::Temperature {
        convert_temperature(value, &source.code, &target.code)
    } else {
        let src_factor = source
            .to_base_factor
            .as_ref()
            .expect("non-temperature units have a factor");
        let tgt_factor = target
            .to_base_factor
            .as_ref()
            .expect("non-temperature units have a factor");
        let product = mul(value, src_factor);
        Ok(div_scale(&product, tgt_factor))
    }
}

/// Convert `value`, asserting both units are in `category`.
///
/// # Errors
/// Additional to [`convert`], returns [`UnitError::WrongCategory`] when either
/// unit lives in a different category.
pub fn convert_in_category(
    value: &BigDecimal,
    from: &str,
    to: &str,
    category: UnitCategory,
) -> Result<BigDecimal, UnitError> {
    let source = require_unit(from)?;
    if source.category != category {
        return Err(UnitError::WrongCategory {
            code: source.code.clone(),
            category: category.as_str().to_string(),
        });
    }
    let target = require_unit(to)?;
    if target.category != category {
        return Err(UnitError::WrongCategory {
            code: target.code.clone(),
            category: category.as_str().to_string(),
        });
    }
    convert(value, from, to)
}

/// All 21 categories in Java declaration order.
#[must_use]
pub fn list_categories() -> &'static [UnitCategory] {
    &ALL_CATEGORIES
}

/// All units within `category`, preserving Java registration order.
#[must_use]
pub fn list_units(category: UnitCategory) -> Vec<&'static UnitDefinition> {
    CATEGORY_INDEX
        .get(&category)
        .map(|codes| {
            codes
                .iter()
                .map(|c| UNITS.get(c).expect("indexed code"))
                .collect()
        })
        .unwrap_or_default()
}

/// Look up a unit by its lowercase code (matching is case-insensitive).
#[must_use]
pub fn find_unit(code: &str) -> Option<&'static UnitDefinition> {
    let key = normalize(code);
    UNITS.get(key.as_str())
}

/// Return the multiplicative factor that maps `from` to `to`.
///
/// # Errors
/// [`UnitError::TemperatureFactor`] when the units are temperatures (they
/// require formulas, not a fixed factor). See [`convert`] for the other
/// possible errors.
pub fn conversion_factor(from: &str, to: &str) -> Result<BigDecimal, UnitError> {
    let source = require_unit(from)?;
    let target = require_unit(to)?;
    require_same_category(source, target)?;
    if source.category == UnitCategory::Temperature {
        return Err(UnitError::TemperatureFactor);
    }
    let src = source
        .to_base_factor
        .as_ref()
        .expect("non-temperature units have a factor");
    let tgt = target
        .to_base_factor
        .as_ref()
        .expect("non-temperature units have a factor");
    Ok(div_scale(src, tgt))
}

/// Human-readable explanation of a conversion, matching Java output byte-for-byte.
///
/// # Errors
/// See [`convert`].
pub fn explain_conversion(from: &str, to: &str) -> Result<String, UnitError> {
    let source = require_unit(from)?;
    let target = require_unit(to)?;
    require_same_category(source, target)?;

    if source.category == UnitCategory::Temperature {
        Ok(explain_temperature(&source.code, &target.code))
    } else {
        let factor = conversion_factor(from, to)?;
        Ok(format!(
            "1 {} = {} {}",
            source.name,
            strip_plain(&factor),
            target.name
        ))
    }
}

/// Convert a value in the temperature unit `code` to Celsius.
///
/// # Errors
/// [`UnitError::UnknownTemperatureUnit`] when `code` is not one of
/// `c`, `f`, `k`, or `r`.
pub fn to_celsius(code: &str, value: &BigDecimal) -> Result<BigDecimal, UnitError> {
    match normalize(code).as_str() {
        "c" => Ok(value.clone()),
        "f" => {
            let shifted = sub(value, &THIRTY_TWO);
            let scaled = mul(&shifted, &FIVE);
            Ok(div_scale(&scaled, &NINE))
        }
        "k" => Ok(sub(value, &KELVIN_OFFSET)),
        "r" => {
            let shifted = sub(value, &RANKINE_OFFSET);
            Ok(mul(&shifted, &RANKINE_RATIO))
        }
        _ => Err(UnitError::UnknownTemperatureUnit(code.to_string())),
    }
}

/// Convert Celsius into the temperature unit `code`.
///
/// # Errors
/// [`UnitError::UnknownTemperatureUnit`] when `code` is not one of
/// `c`, `f`, `k`, or `r`.
pub fn from_celsius(code: &str, celsius: &BigDecimal) -> Result<BigDecimal, UnitError> {
    match normalize(code).as_str() {
        "c" => Ok(celsius.clone()),
        "f" => {
            let scaled = mul(celsius, &NINE);
            let divided = div_scale(&scaled, &FIVE);
            Ok(add(&divided, &THIRTY_TWO))
        }
        "k" => Ok(add(celsius, &KELVIN_OFFSET)),
        "r" => {
            let divided = div_scale(celsius, &RANKINE_RATIO);
            Ok(add(&divided, &RANKINE_OFFSET))
        }
        _ => Err(UnitError::UnknownTemperatureUnit(code.to_string())),
    }
}

/// Gas mark (1–10) → Celsius lookup.
///
/// # Errors
/// [`UnitError::InvalidGasMark`] for values outside 1..=10.
pub fn gas_mark_to_celsius(mark: i32) -> Result<BigDecimal, UnitError> {
    for (m, c) in GAS_MARK_TO_C.iter() {
        if *m == mark {
            return Ok(c.clone());
        }
    }
    Err(UnitError::InvalidGasMark(mark))
}

/// Return the closest gas mark to a Celsius temperature.
///
/// Matches Java: iterates the lookup in ascending order and returns the mark
/// with the smallest absolute distance, preferring earlier entries on ties.
///
/// # Errors
/// Never returns an error today but the result type is preserved for parity
/// with future extensions.
pub fn celsius_to_gas_mark(celsius: &BigDecimal) -> Result<i32, UnitError> {
    let mut closest: i32 = 1;
    let mut min_dist: Option<BigDecimal> = None;
    for (mark, c) in GAS_MARK_TO_C.iter() {
        let dist = (celsius - c).abs();
        let replace = match &min_dist {
            None => true,
            Some(current) => dist < *current,
        };
        if replace {
            min_dist = Some(dist);
            closest = *mark;
        }
    }
    Ok(closest)
}

/// Every registered unit in Java declaration order.
#[must_use]
pub fn all_units() -> &'static [&'static UnitDefinition] {
    &UNITS_FLAT
}

// ------------------------------------------------------------------ //
//  Private temperature helpers
// ------------------------------------------------------------------ //

fn convert_temperature(
    value: &BigDecimal,
    source: &str,
    target: &str,
) -> Result<BigDecimal, UnitError> {
    if source == target {
        Ok(value.clone())
    } else {
        let celsius = to_celsius(source, value)?;
        from_celsius(target, &celsius)
    }
}

fn explain_temperature(source: &str, target: &str) -> String {
    if source == target {
        return "Same unit — no conversion needed".to_string();
    }
    let key = format!("{source}->{target}");
    TEMP_FORMULAS
        .get(key.as_str())
        .copied()
        .unwrap_or("Convert via Celsius intermediate")
        .to_string()
}

// ------------------------------------------------------------------ //
//  Tests
// ------------------------------------------------------------------ //

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn bd_test(s: &str) -> BigDecimal {
        BigDecimal::from_str(s).expect("valid decimal")
    }

    /// Compare two BigDecimals after stripping trailing zeros.
    fn eq_plain(actual: &BigDecimal, expected: &str) {
        assert_eq!(
            strip_plain(actual),
            expected,
            "plain strings differ: {actual}"
        );
    }

    #[test]
    fn list_categories_has_21_in_declaration_order() {
        let cats = list_categories();
        assert_eq!(cats.len(), 21);
        assert_eq!(cats[0], UnitCategory::DataStorage);
        assert_eq!(cats[4], UnitCategory::Temperature);
        assert_eq!(cats[20], UnitCategory::Current);
    }

    #[test]
    fn categories_as_str_matches_java_enum() {
        assert_eq!(UnitCategory::DataStorage.as_str(), "DATA_STORAGE");
        assert_eq!(UnitCategory::Length.as_str(), "LENGTH");
        assert_eq!(UnitCategory::DataRate.as_str(), "DATA_RATE");
        assert_eq!(UnitCategory::Current.as_str(), "CURRENT");
    }

    #[test]
    fn category_parse_case_insensitive() {
        assert_eq!(UnitCategory::parse("length").unwrap(), UnitCategory::Length);
        assert_eq!(
            UnitCategory::parse("DATA_STORAGE").unwrap(),
            UnitCategory::DataStorage
        );
        assert!(matches!(
            UnitCategory::parse("not-a-category"),
            Err(UnitError::UnknownCategory(_))
        ));
    }

    #[test]
    fn length_units_match_java_declaration() {
        let codes: Vec<&str> = list_units(UnitCategory::Length)
            .iter()
            .map(|u| u.code.as_str())
            .collect();
        assert_eq!(
            codes,
            vec!["m", "mm", "cm", "km", "in", "ft", "yd", "mi", "nmi"]
        );
    }

    #[test]
    fn km_to_mi_exact() {
        let result = convert(&bd_test("1"), "km", "mi").unwrap();
        // 1000 / 1609.344 with HALF_UP at scale 34
        eq_plain(&result, "0.6213711922373339696174341843633182");
    }

    #[test]
    fn mi_to_km_exact() {
        let result = convert(&bd_test("1"), "mi", "km").unwrap();
        eq_plain(&result, "1.609344");
    }

    #[test]
    fn lb_to_kg_exact() {
        let result = convert(&bd_test("1"), "lb", "kg").unwrap();
        eq_plain(&result, "0.45359237");
    }

    #[test]
    fn kg_to_lb_roundtrip() {
        let one_lb = convert(&bd_test("1"), "lb", "kg").unwrap();
        let back = convert(&one_lb, "kg", "lb").unwrap();
        eq_plain(&back, "1");
    }

    #[test]
    fn l_to_ml() {
        let result = convert(&bd_test("2"), "l", "ml").unwrap();
        eq_plain(&result, "2000");
    }

    #[test]
    fn ml_to_l() {
        let result = convert(&bd_test("1500"), "ml", "l").unwrap();
        eq_plain(&result, "1.5");
    }

    #[test]
    fn celsius_to_fahrenheit() {
        let result = convert(&bd_test("100"), "c", "f").unwrap();
        eq_plain(&result, "212");
    }

    #[test]
    fn fahrenheit_to_celsius() {
        let result = convert(&bd_test("32"), "f", "c").unwrap();
        eq_plain(&result, "0");
    }

    #[test]
    fn celsius_to_kelvin() {
        let result = convert(&bd_test("0"), "c", "k").unwrap();
        eq_plain(&result, "273.15");
    }

    #[test]
    fn kelvin_to_celsius() {
        let result = convert(&bd_test("0"), "k", "c").unwrap();
        eq_plain(&result, "-273.15");
    }

    #[test]
    fn fahrenheit_to_kelvin_via_celsius() {
        let result = convert(&bd_test("32"), "f", "k").unwrap();
        eq_plain(&result, "273.15");
    }

    #[test]
    fn rankine_roundtrip_through_celsius() {
        // 0 C = 491.67 R
        let r = convert(&bd_test("0"), "c", "r").unwrap();
        eq_plain(&r, "491.67");
        let back = convert(&r, "r", "c").unwrap();
        // Due to HALF_UP at scale 34 the round-trip should be exact.
        eq_plain(&back, "0");
    }

    #[test]
    fn gas_mark_to_celsius_lookup() {
        assert_eq!(gas_mark_to_celsius(1).unwrap(), bd_test("140"));
        assert_eq!(gas_mark_to_celsius(4).unwrap(), bd_test("180"));
        assert_eq!(gas_mark_to_celsius(10).unwrap(), bd_test("260"));
    }

    #[test]
    fn gas_mark_out_of_range() {
        assert!(matches!(
            gas_mark_to_celsius(0),
            Err(UnitError::InvalidGasMark(0))
        ));
        assert!(matches!(
            gas_mark_to_celsius(11),
            Err(UnitError::InvalidGasMark(11))
        ));
    }

    #[test]
    fn celsius_to_gas_mark_closest() {
        assert_eq!(celsius_to_gas_mark(&bd_test("140")).unwrap(), 1);
        assert_eq!(celsius_to_gas_mark(&bd_test("180")).unwrap(), 4);
        assert_eq!(celsius_to_gas_mark(&bd_test("210")).unwrap(), 6);
        assert_eq!(celsius_to_gas_mark(&bd_test("260")).unwrap(), 10);
        assert_eq!(celsius_to_gas_mark(&bd_test("1000")).unwrap(), 10);
    }

    #[test]
    fn cross_category_error() {
        let err = convert(&bd_test("1"), "kg", "m").unwrap_err();
        match err {
            UnitError::CrossCategory {
                from_cat, to_cat, ..
            } => {
                assert_eq!(from_cat, "MASS");
                assert_eq!(to_cat, "LENGTH");
            }
            other => panic!("expected CrossCategory, got {other:?}"),
        }
    }

    #[test]
    fn unknown_unit_error() {
        let err = convert(&bd_test("1"), "foo", "bar").unwrap_err();
        assert!(matches!(err, UnitError::UnknownUnit(ref c) if c == "foo"));
    }

    #[test]
    fn conversion_factor_rejects_temperature() {
        let err = conversion_factor("c", "f").unwrap_err();
        assert!(matches!(err, UnitError::TemperatureFactor));
    }

    #[test]
    fn conversion_factor_km_to_mi() {
        let factor = conversion_factor("km", "mi").unwrap();
        eq_plain(&factor, "0.6213711922373339696174341843633182");
    }

    #[test]
    fn explain_linear() {
        let text = explain_conversion("km", "mi").unwrap();
        assert_eq!(
            text,
            "1 kilometer = 0.6213711922373339696174341843633182 mile"
        );
    }

    #[test]
    fn explain_same_temperature() {
        let text = explain_conversion("c", "c").unwrap();
        assert_eq!(text, "Same unit — no conversion needed");
    }

    #[test]
    fn explain_temperature_formula() {
        let text = explain_conversion("c", "f").unwrap();
        assert_eq!(text, "F = C * 9/5 + 32");
    }

    #[test]
    fn convert_in_category_rejects_wrong_category() {
        let err = convert_in_category(&bd_test("1"), "kg", "lb", UnitCategory::Length).unwrap_err();
        assert!(matches!(err, UnitError::WrongCategory { .. }));
    }

    #[test]
    fn convert_in_category_accepts_matching() {
        let result = convert_in_category(&bd_test("1"), "km", "m", UnitCategory::Length).unwrap();
        eq_plain(&result, "1000");
    }

    #[test]
    fn find_unit_case_insensitive() {
        assert_eq!(find_unit("KM").unwrap().code, "km");
        assert!(find_unit("not-a-unit").is_none());
    }

    #[test]
    fn all_units_has_expected_count() {
        // 7 + 9 + 7 + 9 + 4 + 7 + 5 + 7 + 6 + 4 + 6 + 4 + 4 + 5 + 6 + 9 + 4 + 5 + 4 + 4 + 4
        let expected =
            7 + 9 + 7 + 9 + 4 + 7 + 5 + 7 + 6 + 4 + 6 + 4 + 4 + 5 + 6 + 9 + 4 + 5 + 4 + 4 + 4;
        assert_eq!(all_units().len(), expected);
    }

    #[test]
    fn data_storage_uses_binary_multipliers() {
        // 1 kb = 1024 bytes
        let result = convert(&bd_test("1"), "kb", "byte").unwrap();
        eq_plain(&result, "1024");
        // 1 mb = 1048576 bytes
        let result = convert(&bd_test("1"), "mb", "byte").unwrap();
        eq_plain(&result, "1048576");
    }

    #[test]
    fn pressure_psi_matches_derived_factor() {
        // 1 psi = 0.45359237 * 9.80665 / 0.0254^2 Pa, at scale 34 HALF_UP
        let pa = convert(&bd_test("1"), "psi", "pa").unwrap();
        // Expected value computed by the identical Java formula; we validate
        // it's close to 6894.757... (actual exact scale-34 result).
        let rendered = strip_plain(&pa);
        assert!(
            rendered.starts_with("6894.757293168361336722673445"),
            "got {rendered}"
        );
    }

    #[test]
    fn nautical_mile_exact() {
        let result = convert(&bd_test("1"), "nmi", "m").unwrap();
        eq_plain(&result, "1852");
    }

    #[test]
    fn to_celsius_rejects_unknown() {
        assert!(matches!(
            to_celsius("x", &bd_test("1")),
            Err(UnitError::UnknownTemperatureUnit(ref c)) if c == "x"
        ));
    }

    #[test]
    fn conversion_factor_same_unit_is_one() {
        let factor = conversion_factor("m", "m").unwrap();
        eq_plain(&factor, "1");
    }
}
