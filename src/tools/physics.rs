//! Classical physics formulas — kinematics, gravity, optics, thermodynamics.
//!
//! All inputs are decimal strings (SI base units unless noted). Constants:
//! `G = 6.674e-11`, `c = 299_792_458`, `h = 6.626e-34`, `R = 8.314`,
//! `σ = 5.670e-8`. Outputs are formatted via Rust's debug `f64` representation.

use crate::mcp::message::{ErrorCode, Response, error_with_detail};

const TOOL_KINEMATICS: &str = "KINEMATICS";
const TOOL_PROJECTILE_MOTION: &str = "PROJECTILE_MOTION";
const TOOL_NEWTONS_FORCE: &str = "NEWTONS_FORCE";
const TOOL_GRAVITATIONAL_FORCE: &str = "GRAVITATIONAL_FORCE";
const TOOL_DOPPLER_EFFECT: &str = "DOPPLER_EFFECT";
const TOOL_WAVE_LENGTH: &str = "WAVE_LENGTH";
const TOOL_PLANCK_ENERGY: &str = "PLANCK_ENERGY";
const TOOL_IDEAL_GAS_LAW: &str = "IDEAL_GAS_LAW";
const TOOL_HEAT_TRANSFER: &str = "HEAT_TRANSFER";
const TOOL_STEFAN_BOLTZMANN: &str = "STEFAN_BOLTZMANN";
const TOOL_ESCAPE_VELOCITY: &str = "ESCAPE_VELOCITY";
const TOOL_ORBITAL_VELOCITY: &str = "ORBITAL_VELOCITY";

const G: f64 = 6.674e-11;
const H: f64 = 6.626e-34;
const R_GAS: f64 = 8.314_462;
const SIGMA: f64 = 5.670_374_419e-8;

fn parse(tool: &str, label: &str, input: &str) -> Result<f64, String> {
    input.trim().parse::<f64>().map_err(|_| {
        error_with_detail(
            tool,
            ErrorCode::ParseError,
            "value is not a valid number",
            &format!("{label}={input}"),
        )
    })
}

fn fmt(value: f64) -> String {
    format!("{value:?}")
}

/// 1D constant-acceleration kinematics: from initial velocity, acceleration,
/// and time, return final velocity and displacement.
#[must_use]
pub fn kinematics(initial_velocity: &str, acceleration: &str, time: &str) -> String {
    let v0 = match parse(TOOL_KINEMATICS, "initialVelocity", initial_velocity) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let a = match parse(TOOL_KINEMATICS, "acceleration", acceleration) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let t = match parse(TOOL_KINEMATICS, "time", time) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if t < 0.0 {
        return error_with_detail(
            TOOL_KINEMATICS,
            ErrorCode::DomainError,
            "time must be non-negative",
            &format!("time={t}"),
        );
    }
    let v_final = a.mul_add(t, v0);
    let displacement = v0.mul_add(t, 0.5 * a * t * t);
    Response::ok(TOOL_KINEMATICS)
        .field("FINAL_VELOCITY", fmt(v_final))
        .field("DISPLACEMENT", fmt(displacement))
        .build()
}

/// Projectile motion (no air resistance). Inputs: launch speed (m/s),
/// angle (degrees), gravity (default 9.81). Returns range, peak height,
/// and time of flight.
#[must_use]
pub fn projectile_motion(speed: &str, angle_degrees: &str, gravity: &str) -> String {
    let v = match parse(TOOL_PROJECTILE_MOTION, "speed", speed) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let theta = match parse(TOOL_PROJECTILE_MOTION, "angleDegrees", angle_degrees) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let g = match parse(TOOL_PROJECTILE_MOTION, "gravity", gravity) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if v < 0.0 || g <= 0.0 {
        return error_with_detail(
            TOOL_PROJECTILE_MOTION,
            ErrorCode::DomainError,
            "speed must be non-negative and gravity positive",
            &format!("speed={v}, gravity={g}"),
        );
    }
    if !(0.0..=180.0).contains(&theta) {
        // Angles in [0°, 180°] span every physically sensible launch above
        // the horizon (0 = horizontal forward, 90 = straight up, 180 =
        // horizontal backward). Negative or > 180° angles imply a downward
        // launch, which yields a negative time-of-flight — reject instead.
        return error_with_detail(
            TOOL_PROJECTILE_MOTION,
            ErrorCode::DomainError,
            "angle must be in [0, 180] degrees",
            &format!("angleDegrees={theta}"),
        );
    }
    let rad = theta.to_radians();
    let vx = v * rad.cos();
    let vy = v * rad.sin();
    let t_flight = 2.0 * vy / g;
    let range_raw = vx * t_flight;
    // sin(180°) is 1.22e-16 in f64, so the nominal range at θ=90° leaks FP
    // noise like 1.25e-15 instead of the physically exact 0. Clamp anything
    // below a quarter-ulp of the inputs to 0 so "vertical launch" reports 0.
    let range_floor = (v * v / g) * f64::EPSILON * 8.0;
    let range = if range_raw.abs() <= range_floor {
        0.0
    } else {
        range_raw
    };
    let peak = vy * vy / (2.0 * g);
    Response::ok(TOOL_PROJECTILE_MOTION)
        .field("RANGE", fmt(range))
        .field("PEAK_HEIGHT", fmt(peak))
        .field("TIME_OF_FLIGHT", fmt(t_flight))
        .build()
}

#[must_use]
pub fn newtons_force(mass: &str, acceleration: &str) -> String {
    let m = match parse(TOOL_NEWTONS_FORCE, "mass", mass) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let a = match parse(TOOL_NEWTONS_FORCE, "acceleration", acceleration) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if m < 0.0 {
        return error_with_detail(
            TOOL_NEWTONS_FORCE,
            ErrorCode::DomainError,
            "mass must be non-negative",
            &format!("mass={m}"),
        );
    }
    Response::ok(TOOL_NEWTONS_FORCE).result(fmt(m * a)).build()
}

/// Newton's law of universal gravitation: `F = G m1 m2 / r²`.
#[must_use]
pub fn gravitational_force(m1: &str, m2: &str, distance: &str) -> String {
    let m1v = match parse(TOOL_GRAVITATIONAL_FORCE, "m1", m1) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let m2v = match parse(TOOL_GRAVITATIONAL_FORCE, "m2", m2) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let r = match parse(TOOL_GRAVITATIONAL_FORCE, "distance", distance) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if m1v < 0.0 || m2v < 0.0 {
        return error_with_detail(
            TOOL_GRAVITATIONAL_FORCE,
            ErrorCode::DomainError,
            "masses must be non-negative",
            &format!("m1={m1v}, m2={m2v}"),
        );
    }
    if r <= 0.0 {
        return error_with_detail(
            TOOL_GRAVITATIONAL_FORCE,
            ErrorCode::DomainError,
            "distance must be positive",
            &format!("distance={r}"),
        );
    }
    Response::ok(TOOL_GRAVITATIONAL_FORCE)
        .result(fmt(G * m1v * m2v / (r * r)))
        .build()
}

/// Classical (non-relativistic) Doppler shift for sound.
///
/// `mode` is one of `source` (source moving), `observer` (observer moving), or
/// `both`. Speeds in m/s; positive values mean the moving party approaches.
#[must_use]
pub fn doppler_effect(
    source_freq: &str,
    sound_speed: &str,
    source_velocity: &str,
    observer_velocity: &str,
) -> String {
    let f0 = match parse(TOOL_DOPPLER_EFFECT, "sourceFreq", source_freq) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let c_sound = match parse(TOOL_DOPPLER_EFFECT, "soundSpeed", sound_speed) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let vs = match parse(TOOL_DOPPLER_EFFECT, "sourceVelocity", source_velocity) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let vo = match parse(TOOL_DOPPLER_EFFECT, "observerVelocity", observer_velocity) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if c_sound <= 0.0 {
        return error_with_detail(
            TOOL_DOPPLER_EFFECT,
            ErrorCode::DomainError,
            "soundSpeed must be positive",
            &format!("soundSpeed={c_sound}"),
        );
    }
    let denom = c_sound - vs;
    if denom == 0.0 {
        return error_with_detail(
            TOOL_DOPPLER_EFFECT,
            ErrorCode::DomainError,
            "source moves at sound speed — formula undefined",
            &format!("sourceVelocity={vs}, soundSpeed={c_sound}"),
        );
    }
    let f = f0 * (c_sound + vo) / denom;
    Response::ok(TOOL_DOPPLER_EFFECT).result(fmt(f)).build()
}

/// `λ = c / f` (or any wave: λ = v/f). Returns wavelength in meters when c is
/// the speed of light; otherwise in the same length unit as the speed.
#[must_use]
pub fn wave_length(frequency: &str, wave_speed: &str) -> String {
    let f = match parse(TOOL_WAVE_LENGTH, "frequency", frequency) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let v = match parse(TOOL_WAVE_LENGTH, "waveSpeed", wave_speed) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if f <= 0.0 {
        return error_with_detail(
            TOOL_WAVE_LENGTH,
            ErrorCode::DomainError,
            "frequency must be positive",
            &format!("frequency={f}"),
        );
    }
    if v <= 0.0 {
        return error_with_detail(
            TOOL_WAVE_LENGTH,
            ErrorCode::DomainError,
            "waveSpeed must be positive",
            &format!("waveSpeed={v}"),
        );
    }
    Response::ok(TOOL_WAVE_LENGTH).result(fmt(v / f)).build()
}

/// Photon energy `E = hf` in joules.
#[must_use]
pub fn planck_energy(frequency: &str) -> String {
    let f = match parse(TOOL_PLANCK_ENERGY, "frequency", frequency) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if f < 0.0 {
        return error_with_detail(
            TOOL_PLANCK_ENERGY,
            ErrorCode::DomainError,
            "frequency must be non-negative",
            &format!("frequency={f}"),
        );
    }
    Response::ok(TOOL_PLANCK_ENERGY).result(fmt(H * f)).build()
}

fn non_zero_or_err(label: &str, value: f64, solving: &str) -> Result<f64, String> {
    if value == 0.0 {
        Err(error_with_detail(
            TOOL_IDEAL_GAS_LAW,
            ErrorCode::DivisionByZero,
            &format!("{label} must be non-zero when solving for {solving}"),
            &format!("{label}=0"),
        ))
    } else {
        Ok(value)
    }
}

fn positive_or_err(label: &str, value: f64) -> Result<f64, String> {
    if value <= 0.0 {
        Err(error_with_detail(
            TOOL_IDEAL_GAS_LAW,
            ErrorCode::DomainError,
            &format!("{label} must be positive"),
            &format!("{label}={value}"),
        ))
    } else {
        Ok(value)
    }
}

fn non_negative_or_err(label: &str, value: f64) -> Result<f64, String> {
    if value < 0.0 {
        Err(error_with_detail(
            TOOL_IDEAL_GAS_LAW,
            ErrorCode::DomainError,
            &format!("{label} must be non-negative"),
            &format!("{label}={value}"),
        ))
    } else {
        Ok(value)
    }
}

fn solved_response(solving: &str, value: f64) -> String {
    Response::ok(TOOL_IDEAL_GAS_LAW)
        .field("SOLVED_FOR", solving.to_string())
        .field("VALUE", fmt(value))
        .build()
}

/// Ideal gas law `PV = nRT`. `solveFor` is one of `P`, `V`, `n`, `T`.
/// Provide the three known quantities; the unknown is computed.
#[must_use]
pub fn ideal_gas_law(
    pressure: &str,
    volume: &str,
    moles: &str,
    temperature: &str,
    solve_for: &str,
) -> String {
    let key = solve_for.trim().to_uppercase();
    match key.as_str() {
        "P" => match (
            parse(TOOL_IDEAL_GAS_LAW, "volume", volume)
                .and_then(|v| non_zero_or_err("volume", v, "P"))
                .and_then(|v| positive_or_err("volume", v)),
            parse(TOOL_IDEAL_GAS_LAW, "moles", moles).and_then(|v| non_negative_or_err("moles", v)),
            parse(TOOL_IDEAL_GAS_LAW, "temperature", temperature)
                .and_then(|v| positive_or_err("temperature", v)),
        ) {
            (Ok(v), Ok(n), Ok(t)) => solved_response("P", n * R_GAS * t / v),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => e,
        },
        "V" => match (
            parse(TOOL_IDEAL_GAS_LAW, "pressure", pressure)
                .and_then(|v| non_zero_or_err("pressure", v, "V"))
                .and_then(|v| positive_or_err("pressure", v)),
            parse(TOOL_IDEAL_GAS_LAW, "moles", moles).and_then(|v| non_negative_or_err("moles", v)),
            parse(TOOL_IDEAL_GAS_LAW, "temperature", temperature)
                .and_then(|v| positive_or_err("temperature", v)),
        ) {
            (Ok(p), Ok(n), Ok(t)) => solved_response("V", n * R_GAS * t / p),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => e,
        },
        "N" | "MOLES" => match (
            parse(TOOL_IDEAL_GAS_LAW, "pressure", pressure)
                .and_then(|v| positive_or_err("pressure", v)),
            parse(TOOL_IDEAL_GAS_LAW, "volume", volume).and_then(|v| positive_or_err("volume", v)),
            parse(TOOL_IDEAL_GAS_LAW, "temperature", temperature)
                .and_then(|v| non_zero_or_err("temperature", v, "n"))
                .and_then(|v| positive_or_err("temperature", v)),
        ) {
            (Ok(p), Ok(v), Ok(t)) => solved_response("n", p * v / (R_GAS * t)),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => e,
        },
        "T" => match (
            parse(TOOL_IDEAL_GAS_LAW, "pressure", pressure)
                .and_then(|v| positive_or_err("pressure", v)),
            parse(TOOL_IDEAL_GAS_LAW, "volume", volume).and_then(|v| positive_or_err("volume", v)),
            parse(TOOL_IDEAL_GAS_LAW, "moles", moles)
                .and_then(|v| non_zero_or_err("moles", v, "T"))
                .and_then(|v| positive_or_err("moles", v)),
        ) {
            (Ok(p), Ok(v), Ok(n)) => solved_response("T", p * v / (n * R_GAS)),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => e,
        },
        other => error_with_detail(
            TOOL_IDEAL_GAS_LAW,
            ErrorCode::InvalidInput,
            "solveFor must be one of P, V, n, T",
            &format!("solveFor={other}"),
        ),
    }
}

/// Conduction heat-transfer rate via Fourier's law:
/// `Q = k * A * ΔT / thickness`.
#[must_use]
pub fn heat_transfer(
    thermal_conductivity: &str,
    area: &str,
    delta_temp: &str,
    thickness: &str,
) -> String {
    let k = match parse(
        TOOL_HEAT_TRANSFER,
        "thermalConductivity",
        thermal_conductivity,
    ) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let a = match parse(TOOL_HEAT_TRANSFER, "area", area) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let dt = match parse(TOOL_HEAT_TRANSFER, "deltaTemp", delta_temp) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let l = match parse(TOOL_HEAT_TRANSFER, "thickness", thickness) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if k < 0.0 {
        return error_with_detail(
            TOOL_HEAT_TRANSFER,
            ErrorCode::DomainError,
            "thermalConductivity must be non-negative",
            &format!("thermalConductivity={k}"),
        );
    }
    if a < 0.0 {
        return error_with_detail(
            TOOL_HEAT_TRANSFER,
            ErrorCode::DomainError,
            "area must be non-negative",
            &format!("area={a}"),
        );
    }
    if l <= 0.0 {
        return error_with_detail(
            TOOL_HEAT_TRANSFER,
            ErrorCode::DomainError,
            "thickness must be positive",
            &format!("thickness={l}"),
        );
    }
    Response::ok(TOOL_HEAT_TRANSFER)
        .result(fmt(k * a * dt / l))
        .build()
}

/// Stefan-Boltzmann law: `P = σ * ε * A * T⁴`. Returns radiated power (W).
#[must_use]
pub fn stefan_boltzmann(emissivity: &str, area: &str, temperature_k: &str) -> String {
    let eps = match parse(TOOL_STEFAN_BOLTZMANN, "emissivity", emissivity) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let a = match parse(TOOL_STEFAN_BOLTZMANN, "area", area) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let t = match parse(TOOL_STEFAN_BOLTZMANN, "temperatureK", temperature_k) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !(0.0..=1.0).contains(&eps) {
        return error_with_detail(
            TOOL_STEFAN_BOLTZMANN,
            ErrorCode::OutOfRange,
            "emissivity must be in [0, 1]",
            &format!("emissivity={eps}"),
        );
    }
    if t < 0.0 {
        return error_with_detail(
            TOOL_STEFAN_BOLTZMANN,
            ErrorCode::DomainError,
            "absolute temperature must be non-negative",
            &format!("temperatureK={t}"),
        );
    }
    Response::ok(TOOL_STEFAN_BOLTZMANN)
        .result(fmt(SIGMA * eps * a * t.powi(4)))
        .build()
}

/// Escape velocity from a body of mass M at radius r: `v = √(2GM/r)`.
#[must_use]
pub fn escape_velocity(mass: &str, radius: &str) -> String {
    let m = match parse(TOOL_ESCAPE_VELOCITY, "mass", mass) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let r = match parse(TOOL_ESCAPE_VELOCITY, "radius", radius) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if r <= 0.0 || m < 0.0 {
        return error_with_detail(
            TOOL_ESCAPE_VELOCITY,
            ErrorCode::DomainError,
            "mass must be non-negative and radius positive",
            &format!("mass={m}, radius={r}"),
        );
    }
    Response::ok(TOOL_ESCAPE_VELOCITY)
        .result(fmt((2.0 * G * m / r).sqrt()))
        .build()
}

/// Circular orbital velocity at radius r: `v = √(GM/r)`.
#[must_use]
pub fn orbital_velocity(mass: &str, radius: &str) -> String {
    let m = match parse(TOOL_ORBITAL_VELOCITY, "mass", mass) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let r = match parse(TOOL_ORBITAL_VELOCITY, "radius", radius) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if r <= 0.0 || m < 0.0 {
        return error_with_detail(
            TOOL_ORBITAL_VELOCITY,
            ErrorCode::DomainError,
            "mass must be non-negative and radius positive",
            &format!("mass={m}, radius={r}"),
        );
    }
    Response::ok(TOOL_ORBITAL_VELOCITY)
        .result(fmt((G * m / r).sqrt()))
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
    fn kinematics_simple() {
        // v0=0, a=10, t=2 → vf=20, d=20
        let out = kinematics("0", "10", "2");
        approx_field(&out, "FINAL_VELOCITY", 20.0, 1e-9);
        approx_field(&out, "DISPLACEMENT", 20.0, 1e-9);
    }

    #[test]
    fn projectile_45deg_optimal_range() {
        // v=10, θ=45°, g=9.81 → range = v²/g = 100/9.81 ≈ 10.19
        let out = projectile_motion("10", "45", "9.81");
        approx_field(&out, "RANGE", 100.0 / 9.81, 1e-3);
    }

    #[test]
    fn projectile_negative_speed_errors() {
        let out = projectile_motion("-1", "30", "9.81");
        assert!(out.starts_with("PROJECTILE_MOTION: ERROR"));
    }

    #[test]
    fn newtons_force_basic() {
        // F = ma = 5*2 = 10
        let out = newtons_force("5", "2");
        assert!(out.contains("RESULT: 10.0"), "got {out}");
    }

    #[test]
    fn gravitational_force_two_kg_one_meter() {
        // F = G*1*1/1² = G
        let out = gravitational_force("1", "1", "1");
        approx_field(&out, "RESULT", G, 1e-15);
    }

    #[test]
    fn gravitational_zero_distance_errors() {
        let out = gravitational_force("1", "1", "0");
        assert!(out.starts_with("GRAVITATIONAL_FORCE: ERROR"));
    }

    #[test]
    fn doppler_observer_approaching() {
        // Source at rest, observer moves toward it at half sound speed
        // f = f0 * (c + vo) / c
        let out = doppler_effect("440", "340", "0", "170");
        approx_field(&out, "RESULT", 440.0 * 510.0 / 340.0, 1e-6);
    }

    #[test]
    fn wave_length_speed_of_light_at_1_mhz() {
        // λ = c/f = 3e8 / 1e6 = 300
        let out = wave_length("1000000", "300000000");
        approx_field(&out, "RESULT", 300.0, 1e-6);
    }

    #[test]
    fn planck_energy_basic() {
        // E = h * f for f=1 Hz = h
        let out = planck_energy("1");
        approx_field(&out, "RESULT", H, 1e-40);
    }

    #[test]
    fn ideal_gas_solve_for_v() {
        // PV = nRT → V = nRT/P; n=1, T=273.15, P=101325 → V ≈ 0.02241 m³
        let out = ideal_gas_law("101325", "0", "1", "273.15", "V");
        approx_field(&out, "VALUE", 0.022_413_968, 1e-6);
    }

    #[test]
    fn ideal_gas_invalid_solve_for_errors() {
        let out = ideal_gas_law("1", "1", "1", "1", "Z");
        assert!(out.starts_with("IDEAL_GAS_LAW: ERROR"));
    }

    #[test]
    fn heat_transfer_basic() {
        // k=10, A=1, ΔT=20, L=2 → Q = 10*1*20/2 = 100
        let out = heat_transfer("10", "1", "20", "2");
        approx_field(&out, "RESULT", 100.0, 1e-9);
    }

    #[test]
    fn stefan_boltzmann_blackbody() {
        // ε=1, A=1, T=300 → P = σ * 1 * 300^4 ≈ 459.3 W
        let out = stefan_boltzmann("1", "1", "300");
        approx_field(&out, "RESULT", SIGMA * 300.0_f64.powi(4), 1e-6);
    }

    #[test]
    fn stefan_boltzmann_emissivity_out_of_range() {
        let out = stefan_boltzmann("2", "1", "300");
        assert!(out.starts_with("STEFAN_BOLTZMANN: ERROR"));
    }

    #[test]
    fn escape_velocity_earth_approx() {
        // M_earth = 5.972e24 kg, r = 6.371e6 m → v ≈ 11186 m/s
        let out = escape_velocity("5.972e24", "6.371e6");
        approx_field(&out, "RESULT", 11186.0, 5.0);
    }

    #[test]
    fn orbital_velocity_iss_altitude() {
        // M_earth, r ≈ 6.78e6 → v ≈ 7660 m/s
        let out = orbital_velocity("5.972e24", "6.78e6");
        approx_field(&out, "RESULT", 7660.0, 50.0);
    }

    #[test]
    fn kinematics_rejects_negative_time() {
        let out = kinematics("0", "9.81", "-5");
        assert!(
            out.starts_with("KINEMATICS: ERROR"),
            "expected error, got {out}"
        );
        assert!(out.contains("time must be non-negative"));
    }

    #[test]
    fn kinematics_accepts_negative_acceleration() {
        // Free-fall upward: a=-9.81 is a valid vector; t>=0 only
        let out = kinematics("0", "-9.81", "10");
        assert!(out.starts_with("KINEMATICS: OK"));
    }

    #[test]
    fn newtons_force_rejects_negative_mass() {
        let out = newtons_force("-5", "10");
        assert!(out.starts_with("NEWTONS_FORCE: ERROR"));
        assert!(out.contains("mass must be non-negative"));
    }

    #[test]
    fn newtons_force_zero_mass_ok() {
        let out = newtons_force("0", "10");
        assert!(out.contains("RESULT: 0.0"));
    }

    #[test]
    fn gravitational_force_rejects_negative_mass() {
        let out = gravitational_force("-1000", "1000", "1");
        assert!(out.starts_with("GRAVITATIONAL_FORCE: ERROR"));
        assert!(out.contains("masses must be non-negative"));
    }

    #[test]
    fn planck_energy_rejects_negative_frequency() {
        let out = planck_energy("-1000");
        assert!(out.starts_with("PLANCK_ENERGY: ERROR"));
        assert!(out.contains("frequency must be non-negative"));
    }

    #[test]
    fn planck_energy_zero_frequency_ok() {
        let out = planck_energy("0");
        assert!(out.contains("RESULT: 0.0"));
    }

    #[test]
    fn wave_length_rejects_negative_frequency() {
        let out = wave_length("-1000", "340");
        assert!(out.starts_with("WAVE_LENGTH: ERROR"));
        assert!(out.contains("frequency must be positive"));
    }

    #[test]
    fn wave_length_rejects_negative_wave_speed() {
        let out = wave_length("1000", "-340");
        assert!(out.starts_with("WAVE_LENGTH: ERROR"));
        assert!(out.contains("waveSpeed must be positive"));
    }

    #[test]
    fn heat_transfer_rejects_negative_conductivity() {
        let out = heat_transfer("-5", "1", "20", "1");
        assert!(out.starts_with("HEAT_TRANSFER: ERROR"));
        assert!(out.contains("thermalConductivity must be non-negative"));
    }

    #[test]
    fn heat_transfer_rejects_negative_area() {
        let out = heat_transfer("10", "-1", "20", "1");
        assert!(out.starts_with("HEAT_TRANSFER: ERROR"));
        assert!(out.contains("area must be non-negative"));
    }

    #[test]
    fn ideal_gas_rejects_negative_pressure_when_solving_v() {
        let out = ideal_gas_law("-100", "1", "1", "300", "V");
        assert!(out.starts_with("IDEAL_GAS_LAW: ERROR"));
        assert!(out.contains("pressure must be positive"));
    }

    #[test]
    fn ideal_gas_rejects_negative_volume_when_solving_n() {
        let out = ideal_gas_law("100", "-1", "1", "300", "n");
        assert!(out.starts_with("IDEAL_GAS_LAW: ERROR"));
        assert!(out.contains("volume must be positive"));
    }

    #[test]
    fn ideal_gas_rejects_zero_temperature_when_solving_p() {
        let out = ideal_gas_law("100", "1", "1", "0", "P");
        assert!(out.starts_with("IDEAL_GAS_LAW: ERROR"));
        assert!(out.contains("temperature must be positive"));
    }

    #[test]
    fn ideal_gas_rejects_negative_moles_when_solving_p() {
        let out = ideal_gas_law("100", "1", "-1", "300", "P");
        assert!(out.starts_with("IDEAL_GAS_LAW: ERROR"));
        assert!(out.contains("moles must be non-negative"));
    }

    #[test]
    fn projectile_rejects_negative_angle() {
        let out = projectile_motion("10", "-45", "9.81");
        assert!(out.starts_with("PROJECTILE_MOTION: ERROR"));
        assert!(out.contains("angle must be in [0, 180]"));
    }

    #[test]
    fn projectile_rejects_angle_above_180() {
        let out = projectile_motion("10", "200", "9.81");
        assert!(out.starts_with("PROJECTILE_MOTION: ERROR"));
    }

    #[test]
    fn projectile_backward_launch_signed_range() {
        // 135° is the mirror of 45°: same peak/time, range flipped to negative.
        let back = projectile_motion("10", "135", "9.81");
        let fwd = projectile_motion("10", "45", "9.81");
        assert!(back.starts_with("PROJECTILE_MOTION: OK"));
        // RANGE at 135° ≈ -1 · RANGE at 45° (same magnitude, opposite sign).
        assert!(
            back.contains("RANGE: -"),
            "expected backward range, got {back}"
        );
        assert!(fwd.contains("RANGE: ") && !fwd.contains("RANGE: -"));
    }

    #[test]
    fn projectile_vertical_launch_zero_range() {
        // At 90° the projectile goes straight up — range must be exactly 0.
        let out = projectile_motion("10", "90", "9.81");
        assert!(out.contains("RANGE: 0.0"), "got {out}");
    }
}
