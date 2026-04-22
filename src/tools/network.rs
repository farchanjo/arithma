//! Network tooling — subnet, IP conversion, VLSM, summarization, IPv6
//! compress/expand, and transfer-time / throughput helpers.
//!
//! Every public function returns `String`. Results use the response envelope
//! from `crate::mcp::message` (inline by default, block for VLSM-style tables).

use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use bigdecimal::{BigDecimal, Context, RoundingMode};
use num_bigint::BigInt;
use num_traits::{One, Zero};

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
    if version == 6 {
        match BigInt::from_str(decimal) {
            Ok(big) => Response::ok(DECIMAL_TO_IP)
                .result(big_int_to_ipv6_full(&big))
                .build(),
            Err(_) => error_with_detail(
                DECIMAL_TO_IP,
                ErrorCode::ParseError,
                "decimal is not a valid integer",
                &format!("decimal={decimal}"),
            ),
        }
    } else if version == 4 {
        let value = match decimal.parse::<i64>() {
            Ok(v) => v,
            Err(_) => {
                return error_with_detail(
                    DECIMAL_TO_IP,
                    ErrorCode::ParseError,
                    "decimal is not a valid integer",
                    &format!("decimal={decimal}"),
                );
            }
        };
        const IPV4_MAX: i64 = 0xFFFF_FFFF;
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
    let inside = if is_ipv6(address) {
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

fn cidr_to_mask_v4(cidr: u32) -> i64 {
    if cidr == 0 {
        0
    } else {
        (0xFFFF_FFFFu64 << (IPV4_BITS - cidr) & 0xFFFF_FFFFu64) as i64
    }
}

fn ip_class(ip_value: i64) -> &'static str {
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
    let (_, mag) = value.to_bytes_be();
    let mut raw = String::with_capacity(32);
    for byte in &mag {
        raw.push_str(&format!("{byte:02x}"));
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
    if cidr < 0 || cidr > max {
        return Err(error_with_detail(
            tool,
            ErrorCode::OutOfRange,
            &format!("CIDR must be between 0 and {max}"),
            &format!("cidr={cidr}"),
        ));
    }
    Ok(cidr as u32)
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
        .field("IP_CLASS", ip_class(ip_val))
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
    let (first_host, last_host, usable_hosts) = if host_bits == 0 {
        (network.clone(), network.clone(), BigInt::zero())
    } else if host_bits == 1 {
        (network.clone(), last.clone(), BigInt::from(2u32))
    } else {
        (
            &network + BigInt::one(),
            &last - BigInt::one(),
            &host_range - BigInt::one(),
        )
    };
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

fn ipv4_to_binary(address: &str) -> String {
    let value = match parse_ipv4_for(IP_TO_BINARY, address) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let bin = format!(
        "{}.{}.{}.{}",
        to_binary8(((value >> 24) & 0xFF) as u32),
        to_binary8(((value >> 16) & 0xFF) as u32),
        to_binary8(((value >> 8) & 0xFF) as u32),
        to_binary8((value & 0xFF) as u32)
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
        let parsed = match u32::from_str_radix(group, 16) {
            Ok(v) => v,
            Err(_) => {
                return error_with_detail(
                    IP_TO_BINARY,
                    ErrorCode::ParseError,
                    "address is not a valid IPv6 address",
                    &format!("address={address}"),
                );
            }
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
        let group = match i64::from_str_radix(part, 2) {
            Ok(v) => v,
            Err(_) => {
                return error_with_detail(
                    BINARY_TO_IP,
                    ErrorCode::ParseError,
                    "expected 4 dot-separated 8-bit groups",
                    &format!("binary={binary}"),
                );
            }
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
        let group = match u32::from_str_radix(part, 2) {
            Ok(v) => v,
            Err(_) => {
                return error_with_detail(
                    BINARY_TO_IP,
                    ErrorCode::ParseError,
                    "expected 8 colon-separated 16-bit groups",
                    &format!("binary={binary}"),
                );
            }
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
    let mut best_start: isize = -1;
    let mut best_len: usize = 0;
    let mut cur_start: isize = -1;
    let mut cur_len: usize = 0;

    for (idx, group) in groups.iter().enumerate() {
        if *group == "0000" {
            if cur_start < 0 {
                cur_start = idx as isize;
                cur_len = 1;
            } else {
                cur_len += 1;
            }
        } else {
            if cur_len > best_len {
                best_start = cur_start;
                best_len = cur_len;
            }
            cur_start = -1;
            cur_len = 0;
        }
    }
    if cur_len > best_len {
        best_start = cur_start;
        best_len = cur_len;
    }
    build_compressed(&groups, best_start, best_len)
}

fn build_compressed(groups: &[&str], best_start: isize, best_len: usize) -> String {
    if best_len < MIN_COMPRESS_LEN {
        join_trimmed(groups, 0, groups.len())
    } else {
        let start = best_start as usize;
        let left = join_trimmed(groups, 0, start);
        let right = join_trimmed(groups, start + best_len, groups.len());
        format!("{left}::{right}")
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

fn compute_vlsm(network_cidr: &str, host_counts_json: &str) -> String {
    let cidr_parts: Vec<&str> = network_cidr.split('/').collect();
    if cidr_parts.len() != 2 {
        return error_with_detail(
            VLSM_SUBNETS,
            ErrorCode::ParseError,
            "expected network/prefix form",
            &format!("cidr={network_cidr}"),
        );
    }
    let base_network = match parse_ipv4_for(VLSM_SUBNETS, cidr_parts[0]) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let base_cidr: i32 = match cidr_parts[1].parse() {
        Ok(v) => v,
        Err(_) => {
            return error_with_detail(
                VLSM_SUBNETS,
                ErrorCode::ParseError,
                "prefix is not a valid integer",
                &format!("cidr={}", cidr_parts[1]),
            );
        }
    };
    let base_cidr_u = match validate_cidr_for(VLSM_SUBNETS, base_cidr, false) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let base_mask = cidr_to_mask_v4(base_cidr_u);
    let base_end = base_network | (!base_mask & 0xFFFF_FFFF_i64);

    let mut counts = match parse_int_array(VLSM_SUBNETS, host_counts_json) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if counts.is_empty() {
        return error(
            VLSM_SUBNETS,
            ErrorCode::InvalidInput,
            "host counts array must not be empty",
        );
    }
    // Reject zero and negative host counts — the allocator used to silently
    // accept them and produce /31 allocations with nonsensical usable=-1.
    if let Some(&bad) = counts.iter().find(|&&n| n < 1) {
        return error_with_detail(
            VLSM_SUBNETS,
            ErrorCode::InvalidInput,
            "each host count must be a positive integer",
            &format!("hosts={bad}"),
        );
    }
    counts.sort_by(|a, b| b.cmp(a));

    let mut pointer = base_network;
    let mut response = Response::ok(VLSM_SUBNETS)
        .field("COUNT", counts.len().to_string())
        .block();

    for (idx, &needed) in counts.iter().enumerate() {
        let host_bits = ceil_log2(needed + 2);
        let subnet_cidr_i = IPV4_BITS as i32 - host_bits;
        if subnet_cidr_i < base_cidr {
            return error_with_detail(
                VLSM_SUBNETS,
                ErrorCode::InvalidInput,
                &format!("cannot fit {needed} hosts in /{base_cidr}"),
                &format!("hosts={needed}"),
            );
        }
        let subnet_cidr = subnet_cidr_i as u32;

        let sub_mask = cidr_to_mask_v4(subnet_cidr);
        let sub_broadcast = pointer | (!sub_mask & 0xFFFF_FFFF_i64);
        if sub_broadcast > base_end {
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

fn ceil_log2(value: i32) -> i32 {
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
    let mut min_network: i64 = 0xFFFF_FFFF;
    let mut max_broadcast: i64 = 0;
    for cidr in &cidr_list {
        let parts: Vec<&str> = cidr.split('/').collect();
        if parts.len() != 2 {
            return error_with_detail(
                SUMMARIZE_SUBNETS,
                ErrorCode::ParseError,
                "expected network/prefix form",
                &format!("cidr={cidr}"),
            );
        }
        let network = match parse_ipv4_for(SUMMARIZE_SUBNETS, parts[0]) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let prefix: i32 = match parts[1].parse() {
            Ok(v) => v,
            Err(_) => {
                return error_with_detail(
                    SUMMARIZE_SUBNETS,
                    ErrorCode::ParseError,
                    "prefix is not a valid integer",
                    &format!("cidr={}", parts[1]),
                );
            }
        };
        let prefix_u = match validate_cidr_for(SUMMARIZE_SUBNETS, prefix, false) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let mask = cidr_to_mask_v4(prefix_u);
        let broadcast = network | (!mask & 0xFFFF_FFFF_i64);
        if network < min_network {
            min_network = network;
        }
        if broadcast > max_broadcast {
            max_broadcast = broadcast;
        }
    }
    let range = (max_broadcast - min_network + 1) as i32;
    let super_bits = ceil_log2(range);
    let super_cidr = IPV4_BITS as i32 - super_bits;
    let super_mask = cidr_to_mask_v4(super_cidr as u32);
    let super_network = min_network & super_mask;
    let summary = format!("{}/{}", long_to_ipv4_str(super_network), super_cidr);
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

fn compute_transfer_time(
    file_size: &str,
    file_size_unit: &str,
    bandwidth: &str,
    bandwidth_unit: &str,
) -> String {
    let size_unit = file_size_unit.to_ascii_lowercase();
    let bw_unit = bandwidth_unit.to_ascii_lowercase();
    if let Err(e) = require_category_for(
        TRANSFER_TIME,
        &size_unit,
        UnitCategory::DataStorage,
        "fileSizeUnit",
    ) {
        return e;
    }
    if let Err(e) = require_category_for(
        TRANSFER_TIME,
        &bw_unit,
        UnitCategory::DataRate,
        "bandwidthUnit",
    ) {
        return e;
    }

    let size_value = match parse_decimal_for(TRANSFER_TIME, file_size, "fileSize") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let bandwidth_value = match parse_decimal_for(TRANSFER_TIME, bandwidth, "bandwidth") {
        Ok(v) => v,
        Err(e) => return e,
    };

    let size_bytes = match unit_convert_for(TRANSFER_TIME, &size_value, &size_unit, "byte") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let size_bits = mul_ctx(&size_bytes, &BigDecimal::from(BITS_PER_BYTE));
    let bps = match unit_convert_for(TRANSFER_TIME, &bandwidth_value, &bw_unit, "bps") {
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

fn compute_throughput(
    data_size: &str,
    data_size_unit: &str,
    time: &str,
    time_unit: &str,
    output_unit: &str,
) -> String {
    let size_unit = data_size_unit.to_ascii_lowercase();
    let tu = time_unit.to_ascii_lowercase();
    let out_unit = output_unit.to_ascii_lowercase();

    if let Err(e) = require_category_for(
        THROUGHPUT,
        &size_unit,
        UnitCategory::DataStorage,
        "dataSizeUnit",
    ) {
        return e;
    }
    if let Err(e) = require_category_for(THROUGHPUT, &tu, UnitCategory::Time, "timeUnit") {
        return e;
    }
    if let Err(e) =
        require_category_for(THROUGHPUT, &out_unit, UnitCategory::DataRate, "outputUnit")
    {
        return e;
    }

    let size_value = match parse_decimal_for(THROUGHPUT, data_size, "dataSize") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let time_value = match parse_decimal_for(THROUGHPUT, time, "time") {
        Ok(v) => v,
        Err(e) => return e,
    };

    let size_bytes = match unit_convert_for(THROUGHPUT, &size_value, &size_unit, "byte") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let size_bits = mul_ctx(&size_bytes, &BigDecimal::from(BITS_PER_BYTE));
    let seconds = match unit_convert_for(THROUGHPUT, &time_value, &tu, "s") {
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
    let result = match unit_convert_for(THROUGHPUT, &bps, "bps", &out_unit) {
        Ok(v) => v,
        Err(e) => return e,
    };
    Response::ok(THROUGHPUT).field("RATE", strip(&result)).build()
}

fn compute_tcp_throughput(
    bandwidth_mbps: &str,
    rtt_ms: &str,
    window_size_kb: &str,
) -> String {
    let million = BigDecimal::from(1_000_000);
    let thousand = BigDecimal::from(1_000);
    let kilo_bits = BigDecimal::from(8192);

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

    let bw_bps = mul_ctx(&bw, &million);
    if rtt.is_zero() {
        return error(
            TCP_THROUGHPUT,
            ErrorCode::DivisionByZero,
            "rtt must be greater than zero",
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
    fn subnet_calc_ipv6() {
        assert_eq!(
            subnet_calculator("2001:db8::", 64),
            "SUBNET_CALCULATOR: OK | NETWORK: 2001:0db8:0000:0000:0000:0000:0000:0000 | MASK: ffff:ffff:ffff:ffff:0000:0000:0000:0000 | FIRST_HOST: 2001:0db8:0000:0000:0000:0000:0000:0001 | LAST_HOST: 2001:0db8:0000:0000:ffff:ffff:ffff:fffe | USABLE_HOSTS: 18446744073709551614"
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
        assert_eq!(
            transfer_time("1", "gb", "100", "mbps"),
            "TRANSFER_TIME: OK | SECONDS: 85.89934592 | MINUTES: 1.43165576533333333333 | HOURS: 0.02386092942222222222"
        );
    }

    #[test]
    fn throughput_100mb_10s_to_mbps() {
        assert_eq!(
            throughput("100", "mb", "10", "s", "mbps"),
            "THROUGHPUT: OK | RATE: 83.88608"
        );
    }

    #[test]
    fn tcp_throughput_window_limited() {
        assert_eq!(
            tcp_throughput("1000", "100", "64"),
            "TCP_THROUGHPUT: OK | RATE_MBPS: 5.24288"
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
}
