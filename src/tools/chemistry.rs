//! Chemistry — molar mass, pH/pOH, solution concentration, gas-law moles,
//! radioactive decay.
//!
//! Molar mass parses simple chemical formulas (e.g. `H2O`, `Ca(OH)2`,
//! `Fe2(SO4)3`) into element counts and sums atomic weights from a built-in
//! IUPAC table. Polyatomic groups via parentheses are supported one level deep
//! and through arbitrary nesting via the recursive parser.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::mcp::message::{ErrorCode, Response, error_with_detail};

const TOOL_MOLAR_MASS: &str = "MOLAR_MASS";
const TOOL_PH: &str = "PH";
const TOOL_POH: &str = "POH";
const TOOL_MOLARITY: &str = "MOLARITY";
const TOOL_MOLALITY: &str = "MOLALITY";
const TOOL_HENDERSON_HASSELBALCH: &str = "HENDERSON_HASSELBALCH";
const TOOL_HALF_LIFE: &str = "HALF_LIFE";
const TOOL_DECAY_CONSTANT: &str = "DECAY_CONSTANT";
const TOOL_IDEAL_GAS_MOLES: &str = "IDEAL_GAS_MOLES";

const R_GAS: f64 = 8.314_462;
const LN2: f64 = std::f64::consts::LN_2;

static ATOMIC_WEIGHTS: LazyLock<HashMap<&'static str, f64>> = LazyLock::new(|| {
    // IUPAC 2021 standard atomic weights, abridged for the most common 60+
    // elements used in introductory and applied chemistry. Values in g/mol.
    let mut m = HashMap::new();
    let pairs: &[(&str, f64)] = &[
        ("H", 1.008),
        ("He", 4.002_602),
        ("Li", 6.94),
        ("Be", 9.012_183),
        ("B", 10.81),
        ("C", 12.011),
        ("N", 14.007),
        ("O", 15.999),
        ("F", 18.998_403_163),
        ("Ne", 20.1797),
        ("Na", 22.989_769_28),
        ("Mg", 24.305),
        ("Al", 26.981_538_4),
        ("Si", 28.085),
        ("P", 30.973_761_998),
        ("S", 32.06),
        ("Cl", 35.45),
        ("Ar", 39.95),
        ("K", 39.0983),
        ("Ca", 40.078),
        ("Sc", 44.955_908),
        ("Ti", 47.867),
        ("V", 50.9415),
        ("Cr", 51.9961),
        ("Mn", 54.938_044),
        ("Fe", 55.845),
        ("Co", 58.933_194),
        ("Ni", 58.6934),
        ("Cu", 63.546),
        ("Zn", 65.38),
        ("Ga", 69.723),
        ("Ge", 72.630),
        ("As", 74.921_595),
        ("Se", 78.971),
        ("Br", 79.904),
        ("Kr", 83.798),
        ("Rb", 85.4678),
        ("Sr", 87.62),
        ("Y", 88.905_84),
        ("Zr", 91.224),
        ("Nb", 92.906_37),
        ("Mo", 95.95),
        ("Ag", 107.8682),
        ("Cd", 112.414),
        ("In", 114.818),
        ("Sn", 118.710),
        ("Sb", 121.760),
        ("Te", 127.60),
        ("I", 126.904_47),
        ("Xe", 131.293),
        ("Cs", 132.905_451_96),
        ("Ba", 137.327),
        ("Pt", 195.084),
        ("Au", 196.966_569),
        ("Hg", 200.592),
        ("Pb", 207.2),
        ("Bi", 208.980_40),
        ("U", 238.028_91),
    ];
    for (k, v) in pairs {
        m.insert(*k, *v);
    }
    m
});

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

fn fmt(value: f64) -> String {
    format!("{value:?}")
}

/// Recursive-descent parser for chemical formulas. Produces a map of element
/// → count by walking the input character-by-character.
fn parse_formula(formula: &str) -> Result<HashMap<String, u32>, String> {
    let chars: Vec<char> = formula.chars().collect();
    let mut pos = 0;
    let counts = parse_group(&chars, &mut pos, false)?;
    if pos != chars.len() {
        return Err(format!("unexpected character at position {pos}"));
    }
    Ok(counts)
}

fn parse_group(
    chars: &[char],
    pos: &mut usize,
    inside_paren: bool,
) -> Result<HashMap<String, u32>, String> {
    let mut counts: HashMap<String, u32> = HashMap::new();
    while *pos < chars.len() {
        let ch = chars[*pos];
        if ch == '(' {
            *pos += 1;
            let sub = parse_group(chars, pos, true)?;
            if *pos >= chars.len() || chars[*pos] != ')' {
                return Err(format!("missing ')' near position {pos}"));
            }
            *pos += 1;
            let mult = parse_count(chars, pos);
            for (el, n) in sub {
                *counts.entry(el).or_insert(0) += n * mult;
            }
        } else if ch == ')' {
            if !inside_paren {
                return Err(format!("unexpected ')' at position {pos}"));
            }
            return Ok(counts);
        } else if ch.is_ascii_uppercase() {
            let element = parse_element(chars, pos);
            let mult = parse_count(chars, pos);
            *counts.entry(element).or_insert(0) += mult;
        } else if ch.is_whitespace() {
            *pos += 1;
        } else {
            return Err(format!("unexpected '{ch}' at position {pos}"));
        }
    }
    Ok(counts)
}

fn parse_element(chars: &[char], pos: &mut usize) -> String {
    let mut s = String::new();
    s.push(chars[*pos]);
    *pos += 1;
    while *pos < chars.len() && chars[*pos].is_ascii_lowercase() {
        s.push(chars[*pos]);
        *pos += 1;
    }
    s
}

fn parse_count(chars: &[char], pos: &mut usize) -> u32 {
    let start = *pos;
    while *pos < chars.len() && chars[*pos].is_ascii_digit() {
        *pos += 1;
    }
    if start == *pos {
        return 1;
    }
    let token: String = chars[start..*pos].iter().collect();
    token.parse::<u32>().unwrap_or(1)
}

#[must_use]
pub fn molar_mass(formula: &str) -> String {
    let counts = match parse_formula(formula.trim()) {
        Ok(c) => c,
        Err(e) => {
            return error_with_detail(
                TOOL_MOLAR_MASS,
                ErrorCode::ParseError,
                "invalid chemical formula",
                &format!("formula={formula}, error={e}"),
            );
        }
    };
    if counts.is_empty() {
        return error_with_detail(
            TOOL_MOLAR_MASS,
            ErrorCode::InvalidInput,
            "formula contains no elements",
            &format!("formula={formula}"),
        );
    }
    let mut total = 0.0;
    let mut breakdown: Vec<(String, u32, f64)> = Vec::new();
    for (element, n) in &counts {
        let weight = match ATOMIC_WEIGHTS.get(element.as_str()) {
            Some(w) => *w,
            None => {
                return error_with_detail(
                    TOOL_MOLAR_MASS,
                    ErrorCode::UnknownVariable,
                    "unknown element symbol",
                    &format!("element={element}"),
                );
            }
        };
        let contribution = weight * f64::from(*n);
        total += contribution;
        breakdown.push((element.clone(), *n, contribution));
    }
    breakdown.sort_by(|a, b| a.0.cmp(&b.0));
    let parts = breakdown
        .iter()
        .map(|(el, n, c)| format!("{el}{n}={}", fmt(*c)))
        .collect::<Vec<_>>()
        .join(",");
    Response::ok(TOOL_MOLAR_MASS)
        .field("FORMULA", formula.trim())
        .field("MOLAR_MASS_G_MOL", fmt(total))
        .field("BREAKDOWN", parts)
        .build()
}

#[must_use]
pub fn ph(h_concentration: &str) -> String {
    let h = match parse_decimal(TOOL_PH, "hConcentration", h_concentration) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if h <= 0.0 {
        return error_with_detail(
            TOOL_PH,
            ErrorCode::DomainError,
            "hConcentration must be positive (mol/L)",
            &format!("hConcentration={h}"),
        );
    }
    Response::ok(TOOL_PH).result(fmt(-h.log10())).build()
}

#[must_use]
pub fn poh(oh_concentration: &str) -> String {
    let oh = match parse_decimal(TOOL_POH, "ohConcentration", oh_concentration) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if oh <= 0.0 {
        return error_with_detail(
            TOOL_POH,
            ErrorCode::DomainError,
            "ohConcentration must be positive (mol/L)",
            &format!("ohConcentration={oh}"),
        );
    }
    Response::ok(TOOL_POH).result(fmt(-oh.log10())).build()
}

/// Molarity (mol/L) from moles of solute and litres of solution.
#[must_use]
pub fn molarity(moles: &str, volume_litres: &str) -> String {
    let n = match parse_decimal(TOOL_MOLARITY, "moles", moles) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let v = match parse_decimal(TOOL_MOLARITY, "volumeLitres", volume_litres) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if n < 0.0 {
        return error_with_detail(
            TOOL_MOLARITY,
            ErrorCode::DomainError,
            "moles must be non-negative",
            &format!("moles={n}"),
        );
    }
    if v <= 0.0 {
        return error_with_detail(
            TOOL_MOLARITY,
            ErrorCode::DivisionByZero,
            "volumeLitres must be positive",
            &format!("volumeLitres={v}"),
        );
    }
    Response::ok(TOOL_MOLARITY).result(fmt(n / v)).build()
}

/// Molality (mol/kg) from moles of solute and kilograms of solvent.
#[must_use]
pub fn molality(moles: &str, kilograms_solvent: &str) -> String {
    let n = match parse_decimal(TOOL_MOLALITY, "moles", moles) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let kg = match parse_decimal(TOOL_MOLALITY, "kilogramsSolvent", kilograms_solvent) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if n < 0.0 {
        return error_with_detail(
            TOOL_MOLALITY,
            ErrorCode::DomainError,
            "moles must be non-negative",
            &format!("moles={n}"),
        );
    }
    if kg <= 0.0 {
        return error_with_detail(
            TOOL_MOLALITY,
            ErrorCode::DivisionByZero,
            "kilogramsSolvent must be positive",
            &format!("kilogramsSolvent={kg}"),
        );
    }
    Response::ok(TOOL_MOLALITY).result(fmt(n / kg)).build()
}

/// Henderson-Hasselbalch equation: `pH = pKa + log10([A⁻] / [HA])`.
#[must_use]
pub fn henderson_hasselbalch(pka: &str, conjugate_base: &str, weak_acid: &str) -> String {
    let pka_v = match parse_decimal(TOOL_HENDERSON_HASSELBALCH, "pka", pka) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let a_minus = match parse_decimal(TOOL_HENDERSON_HASSELBALCH, "conjugateBase", conjugate_base) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let ha = match parse_decimal(TOOL_HENDERSON_HASSELBALCH, "weakAcid", weak_acid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if a_minus <= 0.0 || ha <= 0.0 {
        return error_with_detail(
            TOOL_HENDERSON_HASSELBALCH,
            ErrorCode::DomainError,
            "concentrations must be positive",
            &format!("conjugateBase={a_minus}, weakAcid={ha}"),
        );
    }
    Response::ok(TOOL_HENDERSON_HASSELBALCH)
        .result(fmt(pka_v + (a_minus / ha).log10()))
        .build()
}

/// Half-life from decay constant: `t½ = ln(2) / λ`.
#[must_use]
pub fn half_life(decay_constant: &str) -> String {
    let lambda = match parse_decimal(TOOL_HALF_LIFE, "decayConstant", decay_constant) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if lambda <= 0.0 {
        return error_with_detail(
            TOOL_HALF_LIFE,
            ErrorCode::DomainError,
            "decayConstant must be positive",
            &format!("decayConstant={lambda}"),
        );
    }
    Response::ok(TOOL_HALF_LIFE)
        .result(fmt(LN2 / lambda))
        .build()
}

/// Decay constant from half-life: `λ = ln(2) / t½`.
#[must_use]
pub fn decay_constant(half_life_value: &str) -> String {
    let t_half = match parse_decimal(TOOL_DECAY_CONSTANT, "halfLife", half_life_value) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if t_half <= 0.0 {
        return error_with_detail(
            TOOL_DECAY_CONSTANT,
            ErrorCode::DomainError,
            "halfLife must be positive",
            &format!("halfLife={t_half}"),
        );
    }
    Response::ok(TOOL_DECAY_CONSTANT)
        .result(fmt(LN2 / t_half))
        .build()
}

/// Moles of an ideal gas from PV = nRT. Pressure in Pa, V in m³, T in K.
#[must_use]
pub fn ideal_gas_moles(pressure_pa: &str, volume_m3: &str, temperature_k: &str) -> String {
    let p = match parse_decimal(TOOL_IDEAL_GAS_MOLES, "pressurePa", pressure_pa) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let v = match parse_decimal(TOOL_IDEAL_GAS_MOLES, "volumeM3", volume_m3) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let t = match parse_decimal(TOOL_IDEAL_GAS_MOLES, "temperatureK", temperature_k) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if p < 0.0 {
        return error_with_detail(
            TOOL_IDEAL_GAS_MOLES,
            ErrorCode::DomainError,
            "pressurePa must be non-negative",
            &format!("pressurePa={p}"),
        );
    }
    if v < 0.0 {
        return error_with_detail(
            TOOL_IDEAL_GAS_MOLES,
            ErrorCode::DomainError,
            "volumeM3 must be non-negative",
            &format!("volumeM3={v}"),
        );
    }
    if t <= 0.0 {
        return error_with_detail(
            TOOL_IDEAL_GAS_MOLES,
            ErrorCode::DomainError,
            "temperatureK must be positive (Kelvin)",
            &format!("temperatureK={t}"),
        );
    }
    Response::ok(TOOL_IDEAL_GAS_MOLES)
        .result(fmt(p * v / (R_GAS * t)))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_field(out: &str, key: &str, expected: f64, tol: f64) {
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
        let v: f64 = value_str.parse().expect("parse");
        assert!(
            (v - expected).abs() < tol,
            "{key}: expected ~{expected} (tol {tol}), got {v}"
        );
    }

    #[test]
    fn molar_mass_water() {
        // H2O ≈ 2*1.008 + 15.999 = 18.015
        let out = molar_mass("H2O");
        approx_field(&out, "MOLAR_MASS_G_MOL", 18.015, 1e-2);
    }

    #[test]
    fn molar_mass_glucose() {
        // C6H12O6 ≈ 6*12.011 + 12*1.008 + 6*15.999 = 180.156
        let out = molar_mass("C6H12O6");
        approx_field(&out, "MOLAR_MASS_G_MOL", 180.156, 1e-2);
    }

    #[test]
    fn molar_mass_calcium_hydroxide_with_parens() {
        // Ca(OH)2 = 40.078 + 2*(15.999+1.008) = 40.078 + 34.014 = 74.092
        let out = molar_mass("Ca(OH)2");
        approx_field(&out, "MOLAR_MASS_G_MOL", 74.092, 1e-2);
    }

    #[test]
    fn molar_mass_iron_sulfate_nested() {
        // Fe2(SO4)3 = 2*55.845 + 3*(32.06 + 4*15.999) = 111.69 + 3*96.056 = 399.858
        let out = molar_mass("Fe2(SO4)3");
        approx_field(&out, "MOLAR_MASS_G_MOL", 399.858, 1e-1);
    }

    #[test]
    fn molar_mass_unknown_element_errors() {
        let out = molar_mass("Xx2");
        assert!(out.starts_with("MOLAR_MASS: ERROR"));
    }

    #[test]
    fn molar_mass_invalid_syntax_errors() {
        let out = molar_mass("(H2");
        assert!(out.starts_with("MOLAR_MASS: ERROR"));
    }

    #[test]
    fn ph_neutral_water() {
        // [H+] = 1e-7 → pH = 7
        let out = ph("0.0000001");
        approx_field(&out, "RESULT", 7.0, 1e-9);
    }

    #[test]
    fn ph_invalid_concentration_errors() {
        let out = ph("0");
        assert!(out.starts_with("PH: ERROR"));
    }

    #[test]
    fn poh_basic() {
        // [OH-] = 1e-3 → pOH = 3
        let out = poh("0.001");
        approx_field(&out, "RESULT", 3.0, 1e-9);
    }

    #[test]
    fn molarity_basic() {
        // 1 mol / 0.5 L = 2 M
        approx_field(&molarity("1", "0.5"), "RESULT", 2.0, 1e-9);
    }

    #[test]
    fn molality_basic() {
        // 1 mol / 0.5 kg = 2 m
        approx_field(&molality("1", "0.5"), "RESULT", 2.0, 1e-9);
    }

    #[test]
    fn molality_rejects_negative_moles() {
        let out = molality("-1", "1");
        assert!(out.starts_with("MOLALITY: ERROR"));
        assert!(out.contains("moles must be non-negative"));
    }

    #[test]
    fn molarity_rejects_negative_moles() {
        let out = molarity("-1", "1");
        assert!(out.starts_with("MOLARITY: ERROR"));
        assert!(out.contains("moles must be non-negative"));
    }

    #[test]
    fn henderson_hasselbalch_equal_concentrations() {
        // pKa=4.76 (acetic acid), [A-]=[HA] → pH = pKa
        approx_field(
            &henderson_hasselbalch("4.76", "1", "1"),
            "RESULT",
            4.76,
            1e-6,
        );
    }

    #[test]
    fn half_life_round_trip_with_decay_constant() {
        // λ from t½=10 → λ ≈ 0.0693
        let out_lambda = decay_constant("10");
        approx_field(&out_lambda, "RESULT", LN2 / 10.0, 1e-9);
        // t½ from λ=0.0693 → ~10
        let out_t = half_life(&format!("{}", LN2 / 10.0));
        approx_field(&out_t, "RESULT", 10.0, 1e-9);
    }

    #[test]
    fn ideal_gas_moles_basic() {
        // 1 atm = 101325 Pa, 22.4 L = 0.0224 m³, 273.15 K → ~1 mol
        let out = ideal_gas_moles("101325", "0.0224", "273.15");
        approx_field(&out, "RESULT", 1.0, 1e-2);
    }

    #[test]
    fn ideal_gas_moles_rejects_negative_pressure() {
        let out = ideal_gas_moles("-100", "1", "300");
        assert!(out.starts_with("IDEAL_GAS_MOLES: ERROR"));
        assert!(out.contains("pressurePa must be non-negative"));
    }

    #[test]
    fn ideal_gas_moles_rejects_negative_volume() {
        let out = ideal_gas_moles("100", "-1", "300");
        assert!(out.starts_with("IDEAL_GAS_MOLES: ERROR"));
        assert!(out.contains("volumeM3 must be non-negative"));
    }
}
