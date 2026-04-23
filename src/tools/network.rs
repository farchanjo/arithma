//! Network tooling — subnet, IP conversion, VLSM, summarization, IPv6
//! compress/expand, and transfer-time / throughput helpers.
//!
//! Every public function returns `String`. Results use the response envelope
//! from `crate::mcp::message` (inline by default, block for VLSM-style tables).

use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use bigdecimal::{BigDecimal, Context, RoundingMode};
use num_bigint::BigInt;
use num_traits::{One, Signed, Zero};

use crate::engine::bigdecimal_ext::{DECIMAL128_PRECISION, strip_plain};
use crate::engine::unit_registry::{UnitCategory, convert as convert_unit, find_unit};
use crate::mcp::message::{ErrorCode, Response, error, error_with_detail};

// ------------------------------------------------------------------ //
//  Tool names
// ------------------------------------------------------------------ //

const SUBNET_CALCULATOR: &str = "SUBNET_CALCULATOR";
const IP_TO_BINARY: &str = "IP_TO_BINARY";
const BINARY_TO_IP: &str = "BINARY_TO_IP";
const IP_TO_DECIMAL: &str = "IP_TO_DECIMAL";
const DECIMAL_TO_IP: &str = "DECIMAL_TO_IP";
const IP_IN_SUBNET: &str = "IP_IN_SUBNET";
const VLSM_SUBNETS: &str = "VLSM_SUBNETS";
const SUMMARIZE_SUBNETS: &str = "SUMMARIZE_SUBNETS";
const EXPAND_IPV6: &str = "EXPAND_IPV6";
const COMPRESS_IPV6: &str = "COMPRESS_IPV6";
const TRANSFER_TIME: &str = "TRANSFER_TIME";
const THROUGHPUT: &str = "THROUGHPUT";
const TCP_THROUGHPUT: &str = "TCP_THROUGHPUT";

// ------------------------------------------------------------------ //
//  Constants
// ------------------------------------------------------------------ //

const IPV4_BITS: u32 = 32;
const IPV6_BITS: u32 = 128;
const OCTET_COUNT: usize = 4;
const IPV6_GROUP_COUNT: usize = 8;
const CIDR_31: u32 = 31;
const BITS_PER_BYTE: i64 = 8;
const MIN_COMPRESS_LEN: usize = 2;
const SCALE: i64 = 20;
const TRUE_STR: &str = "true";
const FALSE_STR: &str = "false";

// ------------------------------------------------------------------ //
//  Arithmetic helpers — DECIMAL128 precision + HALF_UP rounding
// ------------------------------------------------------------------ //

fn decimal128_ctx() -> Context {
    Context::default()
        .with_prec(DECIMAL128_PRECISION)
        .expect("DECIMAL128_PRECISION is non-zero")
        .with_rounding_mode(RoundingMode::HalfUp)
}

fn mul_ctx(a: &BigDecimal, b: &BigDecimal) -> BigDecimal {
    decimal128_ctx().multiply(a, b)
}

fn div_scaled(a: &BigDecimal, b: &BigDecimal) -> BigDecimal {
    let quotient = a / b;
    quotient.with_scale_round(SCALE, RoundingMode::HalfUp)
}

fn strip(value: &BigDecimal) -> String {
    strip_plain(value)
}

// ------------------------------------------------------------------ //
//  Public API
// ------------------------------------------------------------------ //

/// Calculate subnet details and return a single inline envelope describing
/// the network, broadcast, mask, wildcard, first/last host, usable count and
/// IP class.
#[must_use]
pub fn subnet_calculator(address: &str, cidr: i32) -> String {
    if is_ipv6(address) {
        subnet_v6(address, cidr)
    } else {
        subnet_v4(address, cidr)
    }
}

/// Convert an IP address to its binary representation.
#[must_use]
pub fn ip_to_binary(address: &str) -> String {
    if is_ipv6(address) {
        ipv6_to_binary(address)
    } else {
        ipv4_to_binary(address)
    }
}

/// Convert a binary IP representation back to decimal notation.
#[must_use]
pub fn binary_to_ip(binary: &str) -> String {
    if binary.contains(':') {
        binary_to_ipv6(binary)
    } else {
        binary_to_ipv4(binary)
    }
}

/// Convert an IP address to its unsigned decimal integer representation.
#[must_use]
pub fn ip_to_decimal(address: &str) -> String {
    if is_ipv6(address) {
        match parse_ipv6_for(IP_TO_DECIMAL, address) {
            Ok(v) => Response::ok(IP_TO_DECIMAL).result(v.to_string()).build(),
            Err(e) => e,
        }
    } else {
        match parse_ipv4_for(IP_TO_DECIMAL, address) {
            Ok(v) => Response::ok(IP_TO_DECIMAL).result(v.to_string()).build(),
            Err(e) => e,
        }
    }
}

/// Convert an unsigned decimal integer string to an IP address.
#[must_use]
pub fn decimal_to_ip(decimal: &str, version: i32) -> String {
    const IPV4_MAX: i64 = 0xFFFF_FFFF;
    if version == 6 {
        BigInt::from_str(decimal).map_or_else(
            |_| {
                error_with_detail(
                    DECIMAL_TO_IP,
                    ErrorCode::ParseError,
                    "decimal is not a valid integer",
                    &format!("decimal={decimal}"),
                )
            },
            |big| {
                Response::ok(DECIMAL_TO_IP)
                    .result(big_int_to_ipv6_full(&big))
                    .build()
            },
        )
    } else if version == 4 {
        let Ok(value) = decimal.parse::<i64>() else {
            return error_with_detail(
                DECIMAL_TO_IP,
                ErrorCode::ParseError,
                "decimal is not a valid integer",
                &format!("decimal={decimal}"),
            );
        };
        if !(0..=IPV4_MAX).contains(&value) {
            return error_with_detail(
                DECIMAL_TO_IP,
                ErrorCode::OutOfRange,
                "value does not fit in 32-bit unsigned range",
                &format!("decimal={decimal}"),
            );
        }
        Response::ok(DECIMAL_TO_IP)
            .result(long_to_ipv4_str(value))
            .build()
    } else {
        error_with_detail(
            DECIMAL_TO_IP,
            ErrorCode::InvalidInput,
            "version must be 4 or 6",
            &format!("version={version}"),
        )
    }
}

/// Test whether an IP address is within the given subnet.
#[must_use]
pub fn ip_in_subnet(address: &str, network: &str, cidr: i32) -> String {
    // Dispatch by *address* family, but diagnose family mismatch up front —
    // otherwise a v6 address against a v4 network would parrot "address is
    // not a valid IPv6 address" (on the network input it just routed to),
    // which misleads the caller about which field is wrong.
    let addr_v6 = is_ipv6(address);
    let net_v6 = is_ipv6(network);
    if addr_v6 != net_v6 {
        return error_with_detail(
            IP_IN_SUBNET,
            ErrorCode::InvalidInput,
            "address and network must be the same IP family",
            &format!(
                "address={} ({}), network={} ({})",
                address,
                if addr_v6 { "IPv6" } else { "IPv4" },
                network,
                if net_v6 { "IPv6" } else { "IPv4" },
            ),
        );
    }
    let inside = if addr_v6 {
        check_ipv6_in_subnet(address, network, cidr)
    } else {
        check_ipv4_in_subnet(address, network, cidr)
    };
    match inside {
        Ok(flag) => Response::ok(IP_IN_SUBNET).field("IN_SUBNET", flag).build(),
        Err(e) => e,
    }
}

/// VLSM subnet allocation. `host_counts_json` is a JSON array of host counts.
#[must_use]
pub fn vlsm_subnets(network_cidr: &str, host_counts_json: &str) -> String {
    compute_vlsm(network_cidr, host_counts_json)
}

/// Summarize (supernet) a list of subnets into a single CIDR block.
#[must_use]
pub fn summarize_subnets(subnets_json: &str) -> String {
    compute_summary(subnets_json)
}

/// Expand a compressed IPv6 address to its full 8-group form.
#[must_use]
pub fn expand_ipv6(address: &str) -> String {
    match parse_ipv6_for(EXPAND_IPV6, address) {
        Ok(v) => Response::ok(EXPAND_IPV6)
            .result(big_int_to_ipv6_full(&v))
            .build(),
        Err(e) => e,
    }
}

/// Compress an IPv6 address to its shortest canonical form using `::`.
#[must_use]
pub fn compress_ipv6(address: &str) -> String {
    match parse_ipv6_for(COMPRESS_IPV6, address) {
        Ok(v) => Response::ok(COMPRESS_IPV6)
            .result(compress_ipv6_groups(&big_int_to_ipv6_full(&v)))
            .build(),
        Err(e) => e,
    }
}

/// Estimate file transfer time at a given bandwidth.
#[must_use]
pub fn transfer_time(
    file_size: &str,
    file_size_unit: &str,
    bandwidth: &str,
    bandwidth_unit: &str,
) -> String {
    compute_transfer_time(file_size, file_size_unit, bandwidth, bandwidth_unit)
}

/// Calculate data throughput given a payload size and an elapsed time.
#[must_use]
pub fn throughput(
    data_size: &str,
    data_size_unit: &str,
    time: &str,
    time_unit: &str,
    output_unit: &str,
) -> String {
    compute_throughput(data_size, data_size_unit, time, time_unit, output_unit)
}

/// Effective TCP throughput via bandwidth-delay product (Mbps).
#[must_use]
pub fn tcp_throughput(bandwidth_mbps: &str, rtt_ms: &str, window_size_kb: &str) -> String {
    compute_tcp_throughput(bandwidth_mbps, rtt_ms, window_size_kb)
}

// ------------------------------------------------------------------ //
//  IPv4 helpers
// ------------------------------------------------------------------ //

fn parse_ipv4_for(tool: &str, address: &str) -> Result<i64, String> {
    let parts: Vec<&str> = address.split('.').collect();
    if parts.len() != OCTET_COUNT {
        return Err(error_with_detail(
            tool,
            ErrorCode::ParseError,
            "address is not a valid IPv4 address",
            &format!("address={address}"),
        ));
    }
    let mut value: i64 = 0;
    for part in &parts {
        let octet: i32 = part.parse().map_err(|_| {
            error_with_detail(
                tool,
                ErrorCode::ParseError,
                "address is not a valid IPv4 address",
                &format!("address={address}"),
            )
        })?;
        if !(0..=255).contains(&octet) {
            return Err(error_with_detail(
                tool,
                ErrorCode::OutOfRange,
                "IPv4 octet must be in 0..=255",
                &format!("address={address}"),
            ));
        }
        value = (value << 8) | i64::from(octet);
    }
    if Ipv4Addr::from_str(address).is_err() {
        return Err(error_with_detail(
            tool,
            ErrorCode::ParseError,
            "address is not a valid IPv4 address",
            &format!("address={address}"),
        ));
    }
    Ok(value)
}

fn long_to_ipv4_str(value: i64) -> String {
    format!(
        "{}.{}.{}.{}",
        (value >> 24) & 0xFF,
        (value >> 16) & 0xFF,
        (value >> 8) & 0xFF,
        value & 0xFF
    )
}

const fn cidr_to_mask_v4_u32(cidr: u32) -> u32 {
    if cidr == 0 {
        0
    } else {
        // Shift is bounded by cidr ∈ [1, IPV4_BITS], so every right-hand
        // value fits in u32 and the shift never overflows.
        0xFFFF_FFFFu32 << (IPV4_BITS - cidr)
    }
}

fn cidr_to_mask_v4(cidr: u32) -> i64 {
    i64::from(cidr_to_mask_v4_u32(cidr))
}

const fn ip_class(ip_value: i64) -> &'static str {
    let first_octet = ((ip_value >> 24) & 0xFF) as i32;
    if first_octet <= 127 {
        "A"
    } else if first_octet <= 191 {
        "B"
    } else if first_octet <= 223 {
        "C"
    } else if first_octet <= 239 {
        "D"
    } else {
        "E"
    }
}

// ------------------------------------------------------------------ //
//  IPv6 helpers
// ------------------------------------------------------------------ //

fn is_ipv6(address: &str) -> bool {
    address.contains(':')
}

fn parse_ipv6_for(tool: &str, address: &str) -> Result<BigInt, String> {
    let parsed = Ipv6Addr::from_str(address).map_err(|_| {
        error_with_detail(
            tool,
            ErrorCode::ParseError,
            "address is not a valid IPv6 address",
            &format!("address={address}"),
        )
    })?;
    let bits: u128 = parsed.to_bits();
    Ok(BigInt::from(bits))
}

fn big_int_to_ipv6_full(value: &BigInt) -> String {
    use std::fmt::Write as _;
    let (_, mag) = value.to_bytes_be();
    let mut raw = String::with_capacity(32);
    for byte in &mag {
        write!(&mut raw, "{byte:02x}").expect("write to String never fails");
    }
    let hex = if raw.len() >= 32 {
        raw[raw.len() - 32..].to_string()
    } else {
        let mut s = String::with_capacity(32);
        s.push_str(&"0".repeat(32 - raw.len()));
        s.push_str(&raw);
        s
    };
    let mut out = String::with_capacity(39);
    for idx in 0..IPV6_GROUP_COUNT {
        if idx > 0 {
            out.push(':');
        }
        let start = idx * 4;
        out.push_str(&hex[start..start + 4]);
    }
    out
}

fn cidr_to_mask_v6(cidr: u32) -> BigInt {
    let all_ones = (BigInt::one() << IPV6_BITS) - BigInt::one();
    if cidr == 0 {
        BigInt::zero()
    } else {
        let shifted = &all_ones >> cidr;
        let inverted = !shifted;
        inverted & &all_ones
    }
}

// ------------------------------------------------------------------ //
//  Binary helpers
// ------------------------------------------------------------------ //

fn to_binary8(octet: u32) -> String {
    format!("{octet:08b}")
}

fn to_binary16(group: u32) -> String {
    format!("{group:016b}")
}

// ------------------------------------------------------------------ //
//  Validation
// ------------------------------------------------------------------ //

fn validate_cidr_for(tool: &str, cidr: i32, ipv6: bool) -> Result<u32, String> {
    let max: i32 = if ipv6 { 128 } else { 32 };
    u32::try_from(cidr)
        .ok()
        .filter(|v| *v <= max.unsigned_abs())
        .ok_or_else(|| {
            error_with_detail(
                tool,
                ErrorCode::OutOfRange,
                &format!("CIDR must be between 0 and {max}"),
                &format!("cidr={cidr}"),
            )
        })
}

// ------------------------------------------------------------------ //
//  IP-in-subnet checks
// ------------------------------------------------------------------ //

fn check_ipv6_in_subnet(address: &str, network: &str, cidr: i32) -> Result<&'static str, String> {
    let cidr = validate_cidr_for(IP_IN_SUBNET, cidr, true)?;
    let ip_val = parse_ipv6_for(IP_IN_SUBNET, address)?;
    let net_val = parse_ipv6_for(IP_IN_SUBNET, network)?;
    let mask = cidr_to_mask_v6(cidr);
    Ok(if (&ip_val & &mask) == (&net_val & &mask) {
        TRUE_STR
    } else {
        FALSE_STR
    })
}

fn check_ipv4_in_subnet(address: &str, network: &str, cidr: i32) -> Result<&'static str, String> {
    let cidr = validate_cidr_for(IP_IN_SUBNET, cidr, false)?;
    let ip_val = parse_ipv4_for(IP_IN_SUBNET, address)?;
    let net_val = parse_ipv4_for(IP_IN_SUBNET, network)?;
    let mask = cidr_to_mask_v4(cidr);
    Ok(if (ip_val & mask) == (net_val & mask) {
        TRUE_STR
    } else {
        FALSE_STR
    })
}

// ------------------------------------------------------------------ //
//  Subnet calculation (v4)
// ------------------------------------------------------------------ //

fn subnet_v4(address: &str, cidr: i32) -> String {
    let cidr = match validate_cidr_for(SUBNET_CALCULATOR, cidr, false) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let ip_val = match parse_ipv4_for(SUBNET_CALCULATOR, address) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let mask = cidr_to_mask_v4(cidr);
    let network = ip_val & mask;
    let wildcard = !mask & 0xFFFF_FFFF_i64;
    let broadcast = network | wildcard;
    let (first_host, last_host, usable_hosts) = if cidr == IPV4_BITS {
        (network, network, 0_i64)
    } else if cidr == CIDR_31 {
        (network, broadcast, 2_i64)
    } else {
        (network + 1, broadcast - 1, broadcast - network - 1)
    };
    Response::ok(SUBNET_CALCULATOR)
        .field("NETWORK", long_to_ipv4_str(network))
        .field("BROADCAST", long_to_ipv4_str(broadcast))
        .field("MASK", long_to_ipv4_str(mask))
        .field("WILDCARD", long_to_ipv4_str(wildcard))
        .field("FIRST_HOST", long_to_ipv4_str(first_host))
        .field("LAST_HOST", long_to_ipv4_str(last_host))
        .field("USABLE_HOSTS", usable_hosts.to_string())
        // Classful classification runs off the network address, not the
        // caller-supplied host: `192.168.1.5/0` describes the whole IPv4
        // space (network 0.0.0.0, class A), not class C.
        .field("IP_CLASS", ip_class(network))
        .build()
}

// ------------------------------------------------------------------ //
//  Subnet calculation (v6)
// ------------------------------------------------------------------ //

fn subnet_v6(address: &str, cidr: i32) -> String {
    let cidr = match validate_cidr_for(SUBNET_CALCULATOR, cidr, true) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let ip_val = match parse_ipv6_for(SUBNET_CALCULATOR, address) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let mask = cidr_to_mask_v6(cidr);
    let network = &ip_val & &mask;
    let host_bits = IPV6_BITS - cidr;
    let host_range = if host_bits == 0 {
        BigInt::zero()
    } else {
        (BigInt::one() << host_bits) - BigInt::one()
    };
    let last = &network | &host_range;
    // IPv6 has no broadcast and reserves no network identifier — every address
    // in the block is a valid host, so the usable count is the full 2^host_bits
    // and the range starts at the network address itself.
    let usable_hosts = BigInt::one() << host_bits;
    let (first_host, last_host) = (network.clone(), last);
    Response::ok(SUBNET_CALCULATOR)
        .field("NETWORK", big_int_to_ipv6_full(&network))
        .field("MASK", big_int_to_ipv6_full(&mask))
        .field("FIRST_HOST", big_int_to_ipv6_full(&first_host))
        .field("LAST_HOST", big_int_to_ipv6_full(&last_host))
        .field("USABLE_HOSTS", usable_hosts.to_string())
        .build()
}

// ------------------------------------------------------------------ //
//  Binary conversion
// ------------------------------------------------------------------ //

fn octet_of(value: i64, shift: u32) -> u32 {
    // `& 0xFF` always produces a non-negative value in 0..=255, so the
    // round-trip through u32 is guaranteed lossless.
    u32::try_from((value >> shift) & 0xFF).expect("masked octet fits in u32")
}

fn ipv4_to_binary(address: &str) -> String {
    let value = match parse_ipv4_for(IP_TO_BINARY, address) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let bin = format!(
        "{}.{}.{}.{}",
        to_binary8(octet_of(value, 24)),
        to_binary8(octet_of(value, 16)),
        to_binary8(octet_of(value, 8)),
        to_binary8(octet_of(value, 0)),
    );
    Response::ok(IP_TO_BINARY).result(bin).build()
}

fn ipv6_to_binary(address: &str) -> String {
    let value = match parse_ipv6_for(IP_TO_BINARY, address) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let full = big_int_to_ipv6_full(&value);
    let mut out = String::with_capacity(143);
    for (idx, group) in full.split(':').enumerate() {
        if idx > 0 {
            out.push(':');
        }
        let Ok(parsed) = u32::from_str_radix(group, 16) else {
            return error_with_detail(
                IP_TO_BINARY,
                ErrorCode::ParseError,
                "address is not a valid IPv6 address",
                &format!("address={address}"),
            );
        };
        out.push_str(&to_binary16(parsed));
    }
    Response::ok(IP_TO_BINARY).result(out).build()
}

fn binary_to_ipv4(binary: &str) -> String {
    let parts: Vec<&str> = binary.split('.').collect();
    if parts.len() != OCTET_COUNT {
        return error_with_detail(
            BINARY_TO_IP,
            ErrorCode::InvalidInput,
            "expected 4 dot-separated 8-bit groups",
            &format!("binary={binary}"),
        );
    }
    let mut value: i64 = 0;
    for part in &parts {
        let Ok(group) = i64::from_str_radix(part, 2) else {
            return error_with_detail(
                BINARY_TO_IP,
                ErrorCode::ParseError,
                "expected 4 dot-separated 8-bit groups",
                &format!("binary={binary}"),
            );
        };
        value = (value << 8) | group;
    }
    Response::ok(BINARY_TO_IP)
        .result(long_to_ipv4_str(value))
        .build()
}

fn binary_to_ipv6(binary: &str) -> String {
    let parts: Vec<&str> = binary.split(':').collect();
    if parts.len() != IPV6_GROUP_COUNT {
        return error_with_detail(
            BINARY_TO_IP,
            ErrorCode::InvalidInput,
            "expected 8 colon-separated 16-bit groups",
            &format!("binary={binary}"),
        );
    }
    let mut value = BigInt::zero();
    for part in &parts {
        let Ok(group) = u32::from_str_radix(part, 2) else {
            return error_with_detail(
                BINARY_TO_IP,
                ErrorCode::ParseError,
                "expected 8 colon-separated 16-bit groups",
                &format!("binary={binary}"),
            );
        };
        value = (value << 16) | BigInt::from(group);
    }
    Response::ok(BINARY_TO_IP)
        .result(big_int_to_ipv6_full(&value))
        .build()
}

// ------------------------------------------------------------------ //
//  IPv6 compress
// ------------------------------------------------------------------ //

fn compress_ipv6_groups(full: &str) -> String {
    let groups: Vec<&str> = full.split(':').collect();
    let mut best: Option<(usize, usize)> = None;
    let mut cur: Option<(usize, usize)> = None;

    for (idx, group) in groups.iter().enumerate() {
        if *group == "0000" {
            cur = Some(cur.map_or((idx, 1), |(start, len)| (start, len + 1)));
        } else {
            if let Some((start, len)) = cur
                && len > best.map_or(0, |(_, b)| b)
            {
                best = Some((start, len));
            }
            cur = None;
        }
    }
    if let Some((start, len)) = cur
        && len > best.map_or(0, |(_, b)| b)
    {
        best = Some((start, len));
    }
    build_compressed(&groups, best)
}

fn build_compressed(groups: &[&str], best: Option<(usize, usize)>) -> String {
    match best {
        Some((start, len)) if len >= MIN_COMPRESS_LEN => {
            let left = join_trimmed(groups, 0, start);
            let right = join_trimmed(groups, start + len, groups.len());
            format!("{left}::{right}")
        }
        _ => join_trimmed(groups, 0, groups.len()),
    }
}

fn join_trimmed(groups: &[&str], from: usize, end: usize) -> String {
    let mut out = String::new();
    for group in &groups[from..end] {
        if !out.is_empty() {
            out.push(':');
        }
        out.push_str(&trim_leading_zeros(group));
    }
    out
}

fn trim_leading_zeros(group: &str) -> String {
    let trimmed = group.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

// ------------------------------------------------------------------ //
//  VLSM
// ------------------------------------------------------------------ //

struct VlsmPlan {
    base_network: i64,
    base_end: i64,
    base_cidr: u32,
    counts: Vec<i32>,
}

fn parse_vlsm_inputs(network_cidr: &str, host_counts_json: &str) -> Result<VlsmPlan, String> {
    let cidr_parts: Vec<&str> = network_cidr.split('/').collect();
    if cidr_parts.len() != 2 {
        return Err(error_with_detail(
            VLSM_SUBNETS,
            ErrorCode::ParseError,
            "expected network/prefix form",
            &format!("cidr={network_cidr}"),
        ));
    }
    let base_network = parse_ipv4_for(VLSM_SUBNETS, cidr_parts[0])?;
    let base_cidr_raw: i32 = cidr_parts[1].parse().map_err(|_| {
        error_with_detail(
            VLSM_SUBNETS,
            ErrorCode::ParseError,
            "prefix is not a valid integer",
            &format!("cidr={}", cidr_parts[1]),
        )
    })?;
    let base_cidr = validate_cidr_for(VLSM_SUBNETS, base_cidr_raw, false)?;
    let base_mask = cidr_to_mask_v4(base_cidr);
    let base_end = base_network | (!base_mask & 0xFFFF_FFFF_i64);
    let mut counts = parse_int_array(VLSM_SUBNETS, host_counts_json)?;
    if counts.is_empty() {
        return Err(error(
            VLSM_SUBNETS,
            ErrorCode::InvalidInput,
            "host counts array must not be empty",
        ));
    }
    if let Some(&bad) = counts.iter().find(|&&n| n < 1) {
        return Err(error_with_detail(
            VLSM_SUBNETS,
            ErrorCode::InvalidInput,
            "each host count must be a positive integer",
            &format!("hosts={bad}"),
        ));
    }
    counts.sort_by(|a, b| b.cmp(a));
    Ok(VlsmPlan {
        base_network,
        base_end,
        base_cidr,
        counts,
    })
}

fn required_subnet_cidr(needed: i32, base_cidr: u32) -> Result<u32, String> {
    let host_bits = ceil_log2(needed + 2);
    let prefix_bits = i64::from(IPV4_BITS) - i64::from(host_bits);
    if prefix_bits < i64::from(base_cidr) {
        return Err(error_with_detail(
            VLSM_SUBNETS,
            ErrorCode::InvalidInput,
            &format!("cannot fit {needed} hosts in /{base_cidr}"),
            &format!("hosts={needed}"),
        ));
    }
    // prefix_bits is now in [base_cidr, IPV4_BITS], so the conversion is
    // lossless.
    u32::try_from(prefix_bits).map_err(|_| {
        error(
            VLSM_SUBNETS,
            ErrorCode::InvalidInput,
            "subnet CIDR out of range",
        )
    })
}

fn compute_vlsm(network_cidr: &str, host_counts_json: &str) -> String {
    let plan = match parse_vlsm_inputs(network_cidr, host_counts_json) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let mut pointer = plan.base_network;
    let mut response = Response::ok(VLSM_SUBNETS)
        .field("COUNT", plan.counts.len().to_string())
        .block();
    for (idx, &needed) in plan.counts.iter().enumerate() {
        let subnet_cidr = match required_subnet_cidr(needed, plan.base_cidr) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let sub_mask = cidr_to_mask_v4(subnet_cidr);
        let sub_broadcast = pointer | (!sub_mask & 0xFFFF_FFFF_i64);
        if sub_broadcast > plan.base_end {
            return error(
                VLSM_SUBNETS,
                ErrorCode::InvalidInput,
                "address space exhausted",
            );
        }
        let row_label = format!("ROW_{}", idx + 1);
        let usable = sub_broadcast - pointer - 1;
        let row_value = format!(
            "hosts={needed} | cidr={subnet_cidr} | network={} | broadcast={} | usable={usable}",
            long_to_ipv4_str(pointer),
            long_to_ipv4_str(sub_broadcast),
        );
        response = response.field(row_label, row_value);
        pointer = sub_broadcast + 1;
    }
    response.build()
}

const fn ceil_log2(value: i32) -> i32 {
    let mut bits: i32 = 0;
    let mut remaining = value - 1;
    while remaining > 0 {
        remaining >>= 1;
        bits += 1;
    }
    bits
}

// ------------------------------------------------------------------ //
//  Summarize subnets
// ------------------------------------------------------------------ //

fn parse_cidr_entry(entry: &str) -> Result<(u32, u32), String> {
    let parts: Vec<&str> = entry.split('/').collect();
    if parts.len() != 2 {
        return Err(error_with_detail(
            SUMMARIZE_SUBNETS,
            ErrorCode::ParseError,
            "expected network/prefix form",
            &format!("cidr={entry}"),
        ));
    }
    let network_i64 = parse_ipv4_for(SUMMARIZE_SUBNETS, parts[0])?;
    let network = u32::try_from(network_i64 & 0xFFFF_FFFF_i64).expect("IPv4 32-bit mask");
    let prefix_raw: i32 = parts[1].parse().map_err(|_| {
        error_with_detail(
            SUMMARIZE_SUBNETS,
            ErrorCode::ParseError,
            "prefix is not a valid integer",
            &format!("cidr={}", parts[1]),
        )
    })?;
    let prefix = validate_cidr_for(SUMMARIZE_SUBNETS, prefix_raw, false)?;
    Ok((network, prefix))
}

fn summary_range(cidr_list: &[String]) -> Result<(u32, u32, u64), String> {
    let mut min_network: u32 = u32::MAX;
    let mut max_broadcast: u32 = 0;
    let mut total_input_addresses: u64 = 0;
    for cidr in cidr_list {
        let (network, prefix) = parse_cidr_entry(cidr)?;
        let mask = cidr_to_mask_v4_u32(prefix);
        let broadcast = network | !mask;
        if network < min_network {
            min_network = network;
        }
        if broadcast > max_broadcast {
            max_broadcast = broadcast;
        }
        total_input_addresses += 1_u64 << (IPV4_BITS - prefix);
    }
    Ok((min_network, max_broadcast, total_input_addresses))
}

/// Maximum ratio between supernet size and the sum of input address counts.
/// A 4× ceiling means the supernet may waste at most three times the input
/// space (typical of near-adjacent aggregation) before the tool refuses and
/// asks the caller to split the list.
const MAX_SUMMARIZE_WASTE_RATIO: u64 = 4;

fn compute_summary(subnets_json: &str) -> String {
    let cidr_list = match parse_string_array(SUMMARIZE_SUBNETS, subnets_json) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if cidr_list.is_empty() {
        return error(
            SUMMARIZE_SUBNETS,
            ErrorCode::InvalidInput,
            "subnet list must not be empty",
        );
    }
    let (min_network, max_broadcast, input_addresses) = match summary_range(&cidr_list) {
        Ok(v) => v,
        Err(e) => return e,
    };
    // Longest-common-prefix algorithm: count the matching high-order bits of
    // min_network and max_broadcast. That prefix length IS the supernet CIDR.
    let diff: u32 = min_network ^ max_broadcast;
    let super_cidr: u32 = if diff == 0 {
        IPV4_BITS
    } else {
        diff.leading_zeros()
    };
    let supernet_addresses = 1_u64 << (IPV4_BITS - super_cidr);
    // Guard against nonsense aggregations. Two disjoint /24s in unrelated
    // private ranges (10/8 vs 172.16/12) mathematically admit `0.0.0.0/0` as
    // their common supernet — but "the whole internet" is never what the
    // caller asked for. Require the resulting supernet to be within the
    // waste-ratio cap so adjacency is necessary, not just set coverage.
    if supernet_addresses > MAX_SUMMARIZE_WASTE_RATIO.saturating_mul(input_addresses) {
        return error_with_detail(
            SUMMARIZE_SUBNETS,
            ErrorCode::InvalidInput,
            "subnets are not contiguous enough — supernet would include far more addresses than requested",
            &format!(
                "supernet=/{super_cidr} ({supernet_addresses} addrs), input_sum={input_addresses} addrs, waste_ratio={}x",
                supernet_addresses / input_addresses.max(1)
            ),
        );
    }
    let super_mask = cidr_to_mask_v4_u32(super_cidr);
    let super_network = min_network & super_mask;
    let summary = format!(
        "{}/{}",
        long_to_ipv4_str(i64::from(super_network)),
        super_cidr
    );
    Response::ok(SUMMARIZE_SUBNETS).result(summary).build()
}

// ------------------------------------------------------------------ //
//  Transfer time / throughput
// ------------------------------------------------------------------ //

fn require_category_for(
    tool: &str,
    code: &str,
    category: UnitCategory,
    label: &str,
) -> Result<(), String> {
    match find_unit(code) {
        Some(unit) if unit.category == category => Ok(()),
        Some(unit) => Err(error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            &format!(
                "unit '{}' is not in category {} (expected for {})",
                unit.code,
                category.as_str(),
                label
            ),
            &format!("{label}={code}"),
        )),
        None => Err(error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            "unknown unit",
            &format!("{label}={code}"),
        )),
    }
}

fn unit_convert_for(
    tool: &str,
    value: &BigDecimal,
    from: &str,
    to: &str,
) -> Result<BigDecimal, String> {
    convert_unit(value, from, to).map_err(|e| error(tool, ErrorCode::InvalidInput, &e.to_string()))
}

fn parse_decimal_for(tool: &str, input: &str, label: &str) -> Result<BigDecimal, String> {
    BigDecimal::from_str(input).map_err(|_| {
        error_with_detail(
            tool,
            ErrorCode::ParseError,
            &format!("{label} is not a valid decimal number"),
            &format!("{label}={input}"),
        )
    })
}

struct TransferInputs {
    size_value: BigDecimal,
    size_unit: String,
    bandwidth_value: BigDecimal,
    bw_unit: String,
}

fn parse_transfer_inputs(
    file_size: &str,
    file_size_unit: &str,
    bandwidth: &str,
    bandwidth_unit: &str,
) -> Result<TransferInputs, String> {
    let size_unit = file_size_unit.to_ascii_lowercase();
    let bw_unit = bandwidth_unit.to_ascii_lowercase();
    require_category_for(
        TRANSFER_TIME,
        &size_unit,
        UnitCategory::DataStorage,
        "fileSizeUnit",
    )?;
    require_category_for(
        TRANSFER_TIME,
        &bw_unit,
        UnitCategory::DataRate,
        "bandwidthUnit",
    )?;
    let size_value = parse_decimal_for(TRANSFER_TIME, file_size, "fileSize")?;
    let bandwidth_value = parse_decimal_for(TRANSFER_TIME, bandwidth, "bandwidth")?;
    if size_value.is_negative() {
        return Err(error_with_detail(
            TRANSFER_TIME,
            ErrorCode::InvalidInput,
            "file size must not be negative",
            &format!("fileSize={file_size}"),
        ));
    }
    if bandwidth_value.is_zero() || bandwidth_value.is_negative() {
        return Err(error_with_detail(
            TRANSFER_TIME,
            ErrorCode::InvalidInput,
            "bandwidth must be positive",
            &format!("bandwidth={bandwidth}"),
        ));
    }
    Ok(TransferInputs {
        size_value,
        size_unit,
        bandwidth_value,
        bw_unit,
    })
}

fn compute_transfer_time(
    file_size: &str,
    file_size_unit: &str,
    bandwidth: &str,
    bandwidth_unit: &str,
) -> String {
    let inputs = match parse_transfer_inputs(file_size, file_size_unit, bandwidth, bandwidth_unit) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let size_bytes =
        match unit_convert_for(TRANSFER_TIME, &inputs.size_value, &inputs.size_unit, "byte") {
            Ok(v) => v,
            Err(e) => return e,
        };
    let size_bits = mul_ctx(&size_bytes, &BigDecimal::from(BITS_PER_BYTE));
    let bps = match unit_convert_for(
        TRANSFER_TIME,
        &inputs.bandwidth_value,
        &inputs.bw_unit,
        "bps",
    ) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if bps.is_zero() {
        return error(
            TRANSFER_TIME,
            ErrorCode::DivisionByZero,
            "bandwidth must be greater than zero",
        );
    }
    let seconds = div_scaled(&size_bits, &bps);
    let minutes = div_scaled(&seconds, &BigDecimal::from(60));
    let hours = div_scaled(&seconds, &BigDecimal::from(3600));
    Response::ok(TRANSFER_TIME)
        .field("SECONDS", strip(&seconds))
        .field("MINUTES", strip(&minutes))
        .field("HOURS", strip(&hours))
        .build()
}

struct ThroughputInputs {
    size_value: BigDecimal,
    size_unit: String,
    time_value: BigDecimal,
    tu: String,
    out_unit: String,
}

fn parse_throughput_inputs(
    data_size: &str,
    data_size_unit: &str,
    time: &str,
    time_unit: &str,
    output_unit: &str,
) -> Result<ThroughputInputs, String> {
    let size_unit = data_size_unit.to_ascii_lowercase();
    let tu = time_unit.to_ascii_lowercase();
    let out_unit = output_unit.to_ascii_lowercase();
    require_category_for(
        THROUGHPUT,
        &size_unit,
        UnitCategory::DataStorage,
        "dataSizeUnit",
    )?;
    require_category_for(THROUGHPUT, &tu, UnitCategory::Time, "timeUnit")?;
    require_category_for(THROUGHPUT, &out_unit, UnitCategory::DataRate, "outputUnit")?;
    let size_value = parse_decimal_for(THROUGHPUT, data_size, "dataSize")?;
    let time_value = parse_decimal_for(THROUGHPUT, time, "time")?;
    if size_value.is_negative() {
        return Err(error_with_detail(
            THROUGHPUT,
            ErrorCode::InvalidInput,
            "data size must not be negative",
            &format!("dataSize={data_size}"),
        ));
    }
    if time_value.is_zero() || time_value.is_negative() {
        return Err(error_with_detail(
            THROUGHPUT,
            ErrorCode::InvalidInput,
            "time must be positive",
            &format!("time={time}"),
        ));
    }
    Ok(ThroughputInputs {
        size_value,
        size_unit,
        time_value,
        tu,
        out_unit,
    })
}

fn compute_throughput(
    data_size: &str,
    data_size_unit: &str,
    time: &str,
    time_unit: &str,
    output_unit: &str,
) -> String {
    let inputs =
        match parse_throughput_inputs(data_size, data_size_unit, time, time_unit, output_unit) {
            Ok(v) => v,
            Err(e) => return e,
        };
    let size_bytes =
        match unit_convert_for(THROUGHPUT, &inputs.size_value, &inputs.size_unit, "byte") {
            Ok(v) => v,
            Err(e) => return e,
        };
    let size_bits = mul_ctx(&size_bytes, &BigDecimal::from(BITS_PER_BYTE));
    let seconds = match unit_convert_for(THROUGHPUT, &inputs.time_value, &inputs.tu, "s") {
        Ok(v) => v,
        Err(e) => return e,
    };
    if seconds.is_zero() {
        return error(
            THROUGHPUT,
            ErrorCode::DivisionByZero,
            "time must be greater than zero",
        );
    }
    let bps = div_scaled(&size_bits, &seconds);
    let result = match unit_convert_for(THROUGHPUT, &bps, "bps", &inputs.out_unit) {
        Ok(v) => v,
        Err(e) => return e,
    };
    Response::ok(THROUGHPUT)
        .field("RATE", strip(&result))
        .build()
}

fn compute_tcp_throughput(bandwidth_mbps: &str, rtt_ms: &str, window_size_kb: &str) -> String {
    let million = BigDecimal::from(1_000_000);
    let thousand = BigDecimal::from(1_000);
    // SI-aligned: 1 kB = 1000 bytes = 8000 bits. Matches the DATA_STORAGE
    // registry and the SI Mbps/ms bandwidth units already used here.
    let kilo_bits = BigDecimal::from(8000);

    let bw = match parse_decimal_for(TCP_THROUGHPUT, bandwidth_mbps, "bandwidthMbps") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let rtt = match parse_decimal_for(TCP_THROUGHPUT, rtt_ms, "rttMs") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let window = match parse_decimal_for(TCP_THROUGHPUT, window_size_kb, "windowSizeKb") {
        Ok(v) => v,
        Err(e) => return e,
    };

    if bw.is_zero() || bw.is_negative() {
        return error_with_detail(
            TCP_THROUGHPUT,
            ErrorCode::InvalidInput,
            "bandwidth must be positive",
            &format!("bandwidthMbps={bandwidth_mbps}"),
        );
    }
    if window.is_zero() || window.is_negative() {
        return error_with_detail(
            TCP_THROUGHPUT,
            ErrorCode::InvalidInput,
            "window size must be positive",
            &format!("windowSizeKb={window_size_kb}"),
        );
    }

    let bw_bps = mul_ctx(&bw, &million);
    if rtt.is_zero() {
        return error(
            TCP_THROUGHPUT,
            ErrorCode::DivisionByZero,
            "rtt must be greater than zero",
        );
    }
    if rtt.is_negative() {
        return error_with_detail(
            TCP_THROUGHPUT,
            ErrorCode::InvalidInput,
            "rtt must be positive",
            &format!("rttMs={rtt_ms}"),
        );
    }
    let rtt_sec = div_scaled(&rtt, &thousand);
    let window_bits = mul_ctx(&window, &kilo_bits);
    let max_by_window = div_scaled(&window_bits, &rtt_sec);
    let effective = if bw_bps <= max_by_window {
        bw_bps
    } else {
        max_by_window
    };
    let effective_mbps = div_scaled(&effective, &million);
    Response::ok(TCP_THROUGHPUT)
        .field("RATE_MBPS", strip(&effective_mbps))
        .build()
}

// ------------------------------------------------------------------ //
//  JSON array parsing helpers
// ------------------------------------------------------------------ //

fn parse_int_array(tool: &str, json: &str) -> Result<Vec<i32>, String> {
    if let Ok(v) = serde_json::from_str::<Vec<i32>>(json) {
        return Ok(v);
    }
    let trimmed = json.trim();
    if trimmed.len() < 2 || !trimmed.starts_with('[') || !trimmed.ends_with(']') {
        return Err(error_with_detail(
            tool,
            ErrorCode::ParseError,
            "invalid JSON array",
            &format!("json={json}"),
        ));
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    inner
        .split(',')
        .map(|el| {
            el.trim().parse::<i32>().map_err(|_| {
                error_with_detail(
                    tool,
                    ErrorCode::ParseError,
                    "invalid integer",
                    &format!("value={}", el.trim()),
                )
            })
        })
        .collect()
}

fn parse_string_array(tool: &str, json: &str) -> Result<Vec<String>, String> {
    if let Ok(v) = serde_json::from_str::<Vec<String>>(json) {
        return Ok(v);
    }
    let trimmed = json.trim();
    if trimmed.len() < 2 || !trimmed.starts_with('[') || !trimmed.ends_with(']') {
        return Err(error_with_detail(
            tool,
            ErrorCode::ParseError,
            "invalid JSON array",
            &format!("json={json}"),
        ));
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    Ok(inner
        .split(',')
        .map(|el| el.trim().replace('"', ""))
        .collect())
}

// ------------------------------------------------------------------ //
//  Tests
// ------------------------------------------------------------------ //

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subnet_calc_192_168_1_0_slash_24() {
        assert_eq!(
            subnet_calculator("192.168.1.0", 24),
            "SUBNET_CALCULATOR: OK | NETWORK: 192.168.1.0 | BROADCAST: 192.168.1.255 | MASK: 255.255.255.0 | WILDCARD: 0.0.0.255 | FIRST_HOST: 192.168.1.1 | LAST_HOST: 192.168.1.254 | USABLE_HOSTS: 254 | IP_CLASS: C"
        );
    }

    #[test]
    fn subnet_calc_cidr_31_point_to_point() {
        assert_eq!(
            subnet_calculator("10.0.0.0", 31),
            "SUBNET_CALCULATOR: OK | NETWORK: 10.0.0.0 | BROADCAST: 10.0.0.1 | MASK: 255.255.255.254 | WILDCARD: 0.0.0.1 | FIRST_HOST: 10.0.0.0 | LAST_HOST: 10.0.0.1 | USABLE_HOSTS: 2 | IP_CLASS: A"
        );
    }

    #[test]
    fn subnet_calc_slash_zero_classified_by_network() {
        // Regression for #6: 192.168.1.5/0 covers the entire IPv4 space; the
        // class must reflect the network (0.0.0.0 → A), not the host octets.
        let out = subnet_calculator("192.168.1.5", 0);
        assert!(out.contains("NETWORK: 0.0.0.0"));
        assert!(
            out.contains("IP_CLASS: A"),
            "expected class A for /0 network, got: {out}"
        );
    }

    #[test]
    fn subnet_calc_ipv6() {
        // IPv6 has no broadcast, so FIRST_HOST = network, LAST_HOST = last,
        // USABLE = 2^host_bits (here 2^64).
        assert_eq!(
            subnet_calculator("2001:db8::", 64),
            "SUBNET_CALCULATOR: OK | NETWORK: 2001:0db8:0000:0000:0000:0000:0000:0000 | MASK: ffff:ffff:ffff:ffff:0000:0000:0000:0000 | FIRST_HOST: 2001:0db8:0000:0000:0000:0000:0000:0000 | LAST_HOST: 2001:0db8:0000:0000:ffff:ffff:ffff:ffff | USABLE_HOSTS: 18446744073709551616"
        );
    }

    #[test]
    fn subnet_calc_ipv6_slash_128_single_host() {
        // /128 names one address — that address is itself a valid host.
        let out = subnet_calculator("2001:db8::1", 128);
        assert!(out.contains("USABLE_HOSTS: 1"), "got: {out}");
        assert!(
            out.contains("FIRST_HOST: 2001:0db8:0000:0000:0000:0000:0000:0001"),
            "got: {out}"
        );
    }

    #[test]
    fn ip_to_binary_ipv4_roundtrip() {
        assert_eq!(
            ip_to_binary("192.168.1.1"),
            "IP_TO_BINARY: OK | RESULT: 11000000.10101000.00000001.00000001"
        );
        assert_eq!(
            binary_to_ip("11000000.10101000.00000001.00000001"),
            "BINARY_TO_IP: OK | RESULT: 192.168.1.1"
        );
    }

    #[test]
    fn ip_to_binary_ipv6_roundtrip() {
        assert_eq!(
            ip_to_binary("::1"),
            "IP_TO_BINARY: OK | RESULT: 0000000000000000:0000000000000000:0000000000000000:0000000000000000:0000000000000000:0000000000000000:0000000000000000:0000000000000001"
        );
        assert_eq!(
            binary_to_ip(
                "0000000000000000:0000000000000000:0000000000000000:0000000000000000:0000000000000000:0000000000000000:0000000000000000:0000000000000001"
            ),
            "BINARY_TO_IP: OK | RESULT: 0000:0000:0000:0000:0000:0000:0000:0001"
        );
    }

    #[test]
    fn ip_to_decimal_ipv4_roundtrip() {
        assert_eq!(
            ip_to_decimal("192.168.1.1"),
            "IP_TO_DECIMAL: OK | RESULT: 3232235777"
        );
        assert_eq!(
            decimal_to_ip("3232235777", 4),
            "DECIMAL_TO_IP: OK | RESULT: 192.168.1.1"
        );
    }

    #[test]
    fn ip_to_decimal_ipv6_roundtrip() {
        assert_eq!(ip_to_decimal("::1"), "IP_TO_DECIMAL: OK | RESULT: 1");
        assert_eq!(
            decimal_to_ip("1", 6),
            "DECIMAL_TO_IP: OK | RESULT: 0000:0000:0000:0000:0000:0000:0000:0001"
        );
    }

    #[test]
    fn ip_in_subnet_cases() {
        assert_eq!(
            ip_in_subnet("192.168.1.50", "192.168.1.0", 24),
            "IP_IN_SUBNET: OK | IN_SUBNET: true"
        );
        assert_eq!(
            ip_in_subnet("192.168.2.1", "192.168.1.0", 24),
            "IP_IN_SUBNET: OK | IN_SUBNET: false"
        );
        assert_eq!(
            ip_in_subnet("2001:db8::1", "2001:db8::", 64),
            "IP_IN_SUBNET: OK | IN_SUBNET: true"
        );
        assert_eq!(
            ip_in_subnet("2001:dc8::1", "2001:db8::", 64),
            "IP_IN_SUBNET: OK | IN_SUBNET: false"
        );
    }

    #[test]
    fn ip_in_subnet_family_mismatch_is_explicit() {
        // IPv6 address against IPv4 network used to error with
        // "address is not a valid IPv6 address" on the *network* field,
        // which blamed the wrong input. Now reports the mismatch directly.
        let out = ip_in_subnet("2001:db8::1", "192.168.0.0", 24);
        assert!(out.starts_with("IP_IN_SUBNET: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("same IP family"));
        assert!(out.contains("IPv6") && out.contains("IPv4"));
    }

    #[test]
    fn vlsm_basic_slash_24() {
        assert_eq!(
            vlsm_subnets("192.168.1.0/24", "[50, 25, 10]"),
            "VLSM_SUBNETS: OK\nCOUNT: 3\nROW_1: hosts=50 | cidr=26 | network=192.168.1.0 | broadcast=192.168.1.63 | usable=62\nROW_2: hosts=25 | cidr=27 | network=192.168.1.64 | broadcast=192.168.1.95 | usable=30\nROW_3: hosts=10 | cidr=28 | network=192.168.1.96 | broadcast=192.168.1.111 | usable=14"
        );
    }

    #[test]
    fn vlsm_cannot_fit() {
        assert_eq!(
            vlsm_subnets("192.168.1.0/28", "[100]"),
            "VLSM_SUBNETS: ERROR\nREASON: [INVALID_INPUT] cannot fit 100 hosts in /28\nDETAIL: hosts=100"
        );
    }

    #[test]
    fn vlsm_rejects_empty_host_counts() {
        assert_eq!(
            vlsm_subnets("192.168.1.0/24", "[]"),
            "VLSM_SUBNETS: ERROR\nREASON: [INVALID_INPUT] host counts array must not be empty"
        );
    }

    #[test]
    fn vlsm_rejects_zero_host_count() {
        // Regression: previously allocated a /31 for 0 hosts silently.
        assert_eq!(
            vlsm_subnets("192.168.1.0/24", "[0]"),
            "VLSM_SUBNETS: ERROR\nREASON: [INVALID_INPUT] each host count must be a positive integer\nDETAIL: hosts=0"
        );
    }

    #[test]
    fn vlsm_rejects_negative_host_count() {
        // Regression: previously produced `cidr=32 | usable=-1` silently.
        assert_eq!(
            vlsm_subnets("192.168.1.0/24", "[-10]"),
            "VLSM_SUBNETS: ERROR\nREASON: [INVALID_INPUT] each host count must be a positive integer\nDETAIL: hosts=-10"
        );
    }

    #[test]
    fn summarize_two_slash_25_to_slash_24() {
        assert_eq!(
            summarize_subnets("[\"192.168.0.0/25\",\"192.168.0.128/25\"]"),
            "SUMMARIZE_SUBNETS: OK | RESULT: 192.168.0.0/24"
        );
    }

    #[test]
    fn summarize_adjacent_slash_22() {
        assert_eq!(
            summarize_subnets(
                "[\"192.168.0.0/24\",\"192.168.1.0/24\",\"192.168.2.0/24\",\"192.168.3.0/24\"]",
            ),
            "SUMMARIZE_SUBNETS: OK | RESULT: 192.168.0.0/22"
        );
    }

    #[test]
    fn summarize_refuses_disjoint_rfc1918_blocks() {
        // Regression: the three RFC-1918 blocks mathematically admit
        // `0.0.0.0/0` as the common supernet, but that "summary" would
        // include ~4 billion addresses that the caller never listed. The
        // tool now refuses when the supernet is far larger than the input
        // union (4× waste cap).
        let out = summarize_subnets("[\"10.0.0.0/8\",\"172.16.0.0/12\",\"192.168.0.0/16\"]");
        assert!(out.starts_with("SUMMARIZE_SUBNETS: ERROR"), "got: {out}");
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("not contiguous"));
    }

    #[test]
    fn summarize_refuses_two_distant_slash_24s() {
        // 10/8 and 172.16/12 have nothing in common — their LCP supernet is
        // 0.0.0.0/0, and returning that silently was the original N6 bug.
        let out = summarize_subnets("[\"10.0.0.0/24\",\"172.16.0.0/24\"]");
        assert!(out.starts_with("SUMMARIZE_SUBNETS: ERROR"), "got: {out}");
    }

    #[test]
    fn summarize_adjacent_slash_24s_still_succeeds() {
        // 192.168.0.0/24 + 192.168.1.0/24 is the textbook contiguous case —
        // the supernet /23 has the same address count as the inputs combined.
        assert_eq!(
            summarize_subnets("[\"192.168.0.0/24\",\"192.168.1.0/24\"]"),
            "SUMMARIZE_SUBNETS: OK | RESULT: 192.168.0.0/23"
        );
    }

    #[test]
    fn summarize_single_subnet_returns_itself() {
        assert_eq!(
            summarize_subnets("[\"192.168.0.0/24\"]"),
            "SUMMARIZE_SUBNETS: OK | RESULT: 192.168.0.0/24"
        );
    }

    #[test]
    fn expand_compress_ipv6_roundtrip() {
        assert_eq!(
            expand_ipv6("::1"),
            "EXPAND_IPV6: OK | RESULT: 0000:0000:0000:0000:0000:0000:0000:0001"
        );
        assert_eq!(
            compress_ipv6("0000:0000:0000:0000:0000:0000:0000:0001"),
            "COMPRESS_IPV6: OK | RESULT: ::1"
        );
    }

    #[test]
    fn compress_ipv6_middle_run() {
        assert_eq!(
            compress_ipv6("2001:0db8:0000:0000:0000:0000:0000:0001"),
            "COMPRESS_IPV6: OK | RESULT: 2001:db8::1"
        );
    }

    #[test]
    fn transfer_time_1gb_at_100mbps() {
        // SI decimal: 1 GB = 8e9 bits → 80 s at 100 Mbps.
        assert_eq!(
            transfer_time("1", "gb", "100", "mbps"),
            "TRANSFER_TIME: OK | SECONDS: 80 | MINUTES: 1.33333333333333333333 | HOURS: 0.02222222222222222222"
        );
    }

    #[test]
    fn throughput_100mb_10s_to_mbps() {
        // SI decimal: 100 MB = 8e8 bits / 10 s = 80 Mbps.
        assert_eq!(
            throughput("100", "mb", "10", "s", "mbps"),
            "THROUGHPUT: OK | RATE: 80"
        );
    }

    #[test]
    fn tcp_throughput_window_limited() {
        // SI decimal: 64 kB = 512 000 bits / 100 ms = 5.12 Mbps.
        assert_eq!(
            tcp_throughput("1000", "100", "64"),
            "TCP_THROUGHPUT: OK | RATE_MBPS: 5.12"
        );
    }

    #[test]
    fn tcp_throughput_bw_limited() {
        assert_eq!(
            tcp_throughput("10", "10", "1024"),
            "TCP_THROUGHPUT: OK | RATE_MBPS: 10"
        );
    }

    #[test]
    fn tcp_throughput_rejects_negative_bandwidth() {
        assert_eq!(
            tcp_throughput("-100", "50", "64"),
            "TCP_THROUGHPUT: ERROR\nREASON: [INVALID_INPUT] bandwidth must be positive\nDETAIL: bandwidthMbps=-100"
        );
    }

    #[test]
    fn tcp_throughput_rejects_zero_window() {
        assert_eq!(
            tcp_throughput("100", "50", "0"),
            "TCP_THROUGHPUT: ERROR\nREASON: [INVALID_INPUT] window size must be positive\nDETAIL: windowSizeKb=0"
        );
    }

    #[test]
    fn tcp_throughput_rejects_negative_rtt() {
        assert_eq!(
            tcp_throughput("100", "-10", "64"),
            "TCP_THROUGHPUT: ERROR\nREASON: [INVALID_INPUT] rtt must be positive\nDETAIL: rttMs=-10"
        );
    }

    #[test]
    fn error_bad_ip() {
        assert_eq!(
            ip_to_decimal("999.999.999.999"),
            "IP_TO_DECIMAL: ERROR\nREASON: [OUT_OF_RANGE] IPv4 octet must be in 0..=255\nDETAIL: address=999.999.999.999"
        );
    }

    #[test]
    fn error_bad_cidr() {
        assert_eq!(
            subnet_calculator("192.168.1.0", 33),
            "SUBNET_CALCULATOR: ERROR\nREASON: [OUT_OF_RANGE] CIDR must be between 0 and 32\nDETAIL: cidr=33"
        );
    }

    #[test]
    fn error_wrong_version() {
        assert_eq!(
            decimal_to_ip("1", 5),
            "DECIMAL_TO_IP: ERROR\nREASON: [INVALID_INPUT] version must be 4 or 6\nDETAIL: version=5"
        );
    }

    #[test]
    fn error_empty_summary_list() {
        assert_eq!(
            summarize_subnets("[]"),
            "SUMMARIZE_SUBNETS: ERROR\nREASON: [INVALID_INPUT] subnet list must not be empty"
        );
    }

    #[test]
    fn error_binary_to_ipv4_group_count() {
        assert_eq!(
            binary_to_ip("1010.1010"),
            "BINARY_TO_IP: ERROR\nREASON: [INVALID_INPUT] expected 4 dot-separated 8-bit groups\nDETAIL: binary=1010.1010"
        );
    }

    #[test]
    fn transfer_time_rejects_negative_file_size() {
        let out = transfer_time("-1", "gb", "100", "mbps");
        assert!(out.contains("TRANSFER_TIME: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("file size must not be negative"));
    }

    #[test]
    fn transfer_time_rejects_negative_bandwidth() {
        let out = transfer_time("1", "gb", "-100", "mbps");
        assert!(out.contains("TRANSFER_TIME: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("bandwidth must be positive"));
    }

    #[test]
    fn transfer_time_rejects_zero_bandwidth() {
        let out = transfer_time("1", "gb", "0", "mbps");
        assert!(out.contains("TRANSFER_TIME: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("bandwidth must be positive"));
    }

    #[test]
    fn throughput_rejects_negative_data_size() {
        let out = throughput("-500", "mb", "10", "s", "mbps");
        assert!(out.contains("THROUGHPUT: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("data size must not be negative"));
    }

    #[test]
    fn throughput_rejects_negative_time() {
        let out = throughput("500", "mb", "-10", "s", "mbps");
        assert!(out.contains("THROUGHPUT: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("time must be positive"));
    }

    #[test]
    fn throughput_rejects_zero_time() {
        let out = throughput("500", "mb", "0", "s", "mbps");
        assert!(out.contains("THROUGHPUT: ERROR"));
        assert!(out.contains("INVALID_INPUT"));
        assert!(out.contains("time must be positive"));
    }
}
