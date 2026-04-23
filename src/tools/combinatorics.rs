//! Combinatorics and number theory — exact integer arithmetic via `num-bigint`.
//!
//! All results stay in arbitrary precision so e.g. `combination(100, 50)` returns
//! the full 30-digit integer rather than an f64 approximation.

use num_bigint::BigUint;
use num_traits::{One, Zero};

use crate::mcp::message::{ErrorCode, Response, error_with_detail};

const TOOL_COMBINATION: &str = "COMBINATION";
const TOOL_PERMUTATION: &str = "PERMUTATION";
const TOOL_FIBONACCI: &str = "FIBONACCI";
const TOOL_IS_PRIME: &str = "IS_PRIME";
const TOOL_NEXT_PRIME: &str = "NEXT_PRIME";
const TOOL_PRIME_FACTORS: &str = "PRIME_FACTORS";
const TOOL_EULER_TOTIENT: &str = "EULER_TOTIENT";

/// Cap on inputs that would produce results too large to be useful in an MCP
/// envelope. Mirrors the spirit of `MAX_POWER_RESULT_LEN` in basic.rs.
const MAX_N_FOR_COMBINATORIAL: u64 = 10_000;
const MAX_N_FOR_FIBONACCI: u64 = 50_000;

#[must_use]
pub fn combination(n: i64, k: i64) -> String {
    if n < 0 || k < 0 {
        return error_with_detail(
            TOOL_COMBINATION,
            ErrorCode::OutOfRange,
            "n and k must be non-negative",
            &format!("n={n}, k={k}"),
        );
    }
    if k > n {
        // C(n, k) = 0 when k > n
        return Response::ok(TOOL_COMBINATION).result("0").build();
    }
    // Bounds above guarantee n, k >= 0, so `unsigned_abs` is lossless.
    let nu = n.unsigned_abs();
    if nu > MAX_N_FOR_COMBINATORIAL {
        return error_with_detail(
            TOOL_COMBINATION,
            ErrorCode::OutOfRange,
            "n exceeds the safe upper bound",
            &format!("n={n}, max={MAX_N_FOR_COMBINATORIAL}"),
        );
    }
    let mut k_u = k.unsigned_abs();
    // Use the smaller of k and n-k for fewer multiplications.
    if k_u > nu - k_u {
        k_u = nu - k_u;
    }
    let mut result = BigUint::one();
    for i in 0..k_u {
        result *= BigUint::from(nu - i);
        result /= BigUint::from(i + 1);
    }
    Response::ok(TOOL_COMBINATION)
        .result(result.to_string())
        .build()
}

#[must_use]
pub fn permutation(n: i64, k: i64) -> String {
    if n < 0 || k < 0 {
        return error_with_detail(
            TOOL_PERMUTATION,
            ErrorCode::OutOfRange,
            "n and k must be non-negative",
            &format!("n={n}, k={k}"),
        );
    }
    if k > n {
        return Response::ok(TOOL_PERMUTATION).result("0").build();
    }
    let nu = n.unsigned_abs();
    let ku = k.unsigned_abs();
    if nu > MAX_N_FOR_COMBINATORIAL {
        return error_with_detail(
            TOOL_PERMUTATION,
            ErrorCode::OutOfRange,
            "n exceeds the safe upper bound",
            &format!("n={n}, max={MAX_N_FOR_COMBINATORIAL}"),
        );
    }
    let mut result = BigUint::one();
    for i in 0..ku {
        result *= BigUint::from(nu - i);
    }
    Response::ok(TOOL_PERMUTATION)
        .result(result.to_string())
        .build()
}

#[must_use]
pub fn fibonacci(n: i64) -> String {
    if n < 0 {
        return error_with_detail(
            TOOL_FIBONACCI,
            ErrorCode::OutOfRange,
            "n must be non-negative",
            &format!("n={n}"),
        );
    }
    let nu = n.unsigned_abs();
    if nu > MAX_N_FOR_FIBONACCI {
        return error_with_detail(
            TOOL_FIBONACCI,
            ErrorCode::OutOfRange,
            "n exceeds the safe upper bound",
            &format!("n={n}, max={MAX_N_FOR_FIBONACCI}"),
        );
    }
    let result = fib_iterative(nu);
    Response::ok(TOOL_FIBONACCI)
        .result(result.to_string())
        .build()
}

fn fib_iterative(n: u64) -> BigUint {
    if n == 0 {
        return BigUint::zero();
    }
    let mut a = BigUint::zero();
    let mut b = BigUint::one();
    for _ in 1..n {
        let next = &a + &b;
        a = b;
        b = next;
    }
    b
}

#[must_use]
pub fn is_prime(n: i64) -> String {
    if n < 0 {
        return error_with_detail(
            TOOL_IS_PRIME,
            ErrorCode::OutOfRange,
            "primality is only defined for non-negative integers",
            &format!("n={n}"),
        );
    }
    if n < 2 {
        return Response::ok(TOOL_IS_PRIME)
            .field("N", n.to_string())
            .field("IS_PRIME", "false")
            .build();
    }
    let nu = n.unsigned_abs();
    let prime = is_prime_u64(nu);
    Response::ok(TOOL_IS_PRIME)
        .field("N", n.to_string())
        .field("IS_PRIME", if prime { "true" } else { "false" })
        .build()
}

const fn is_prime_u64(n: u64) -> bool {
    if n < 2 {
        return false;
    }
    if n < 4 {
        return true;
    }
    if n.is_multiple_of(2) {
        return false;
    }
    if n.is_multiple_of(3) {
        return n == 3;
    }
    let mut i: u64 = 5;
    while i.saturating_mul(i) <= n {
        if n.is_multiple_of(i) || n.is_multiple_of(i + 2) {
            return false;
        }
        i += 6;
    }
    true
}

#[must_use]
pub fn next_prime(n: i64) -> String {
    if n < 0 {
        return error_with_detail(
            TOOL_NEXT_PRIME,
            ErrorCode::OutOfRange,
            "n must be non-negative",
            &format!("n={n}"),
        );
    }
    let mut candidate: u64 = n.unsigned_abs().saturating_add(1).max(2);
    // Hard cap to avoid runaway searches; a prime exists within 2x of any
    // number by Bertrand's postulate, so this is generous.
    let limit = candidate.saturating_mul(4);
    while candidate <= limit {
        if is_prime_u64(candidate) {
            return Response::ok(TOOL_NEXT_PRIME)
                .result(candidate.to_string())
                .build();
        }
        candidate = candidate.saturating_add(1);
    }
    error_with_detail(
        TOOL_NEXT_PRIME,
        ErrorCode::Overflow,
        "search range exceeded — input too large",
        &format!("n={n}"),
    )
}

/// Trial division — returns the multiset of prime factors as a comma-separated
/// list. Caps `n` at 10^12 to keep response time bounded.
#[must_use]
pub fn prime_factors(n: i64) -> String {
    if n < 1 {
        return error_with_detail(
            TOOL_PRIME_FACTORS,
            ErrorCode::OutOfRange,
            "n must be at least 1",
            &format!("n={n}"),
        );
    }
    let mut value = n.unsigned_abs();
    if value > 1_000_000_000_000 {
        return error_with_detail(
            TOOL_PRIME_FACTORS,
            ErrorCode::OutOfRange,
            "n exceeds the safe upper bound",
            &format!("n={n}, max=1000000000000"),
        );
    }
    // The prime factorization of 1 is the empty product — no primes multiply
    // to give 1. Keep the COUNT=0 / FACTORS= response so callers can branch
    // on presence without special-casing this input (aligns with eulerTotient
    // accepting n=1 and returning 1).
    let mut factors: Vec<u64> = Vec::new();
    let mut divisor: u64 = 2;
    while divisor.saturating_mul(divisor) <= value {
        while value.is_multiple_of(divisor) {
            factors.push(divisor);
            value /= divisor;
        }
        divisor = if divisor == 2 { 3 } else { divisor + 2 };
    }
    if value > 1 {
        factors.push(value);
    }
    let factors_str = factors
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    Response::ok(TOOL_PRIME_FACTORS)
        .field("N", n.to_string())
        .field("FACTORS", factors_str)
        .field("COUNT", factors.len().to_string())
        .build()
}

/// Euler's totient φ(n): count of integers in [1, n] coprime to `n`.
#[must_use]
pub fn euler_totient(n: i64) -> String {
    if n < 1 {
        return error_with_detail(
            TOOL_EULER_TOTIENT,
            ErrorCode::OutOfRange,
            "n must be at least 1",
            &format!("n={n}"),
        );
    }
    let n_u: u64 = match u64::try_from(n) {
        Ok(v) => v,
        Err(_) => {
            return error_with_detail(
                TOOL_EULER_TOTIENT,
                ErrorCode::OutOfRange,
                "n is out of range",
                &format!("n={n}"),
            );
        }
    };
    if n_u == 1 {
        return Response::ok(TOOL_EULER_TOTIENT).result("1").build();
    }
    if n_u > 1_000_000_000_000 {
        return error_with_detail(
            TOOL_EULER_TOTIENT,
            ErrorCode::OutOfRange,
            "n exceeds the safe upper bound",
            &format!("n={n}"),
        );
    }
    let mut result = n_u;
    let mut value = n_u;
    let mut p: u64 = 2;
    while p.saturating_mul(p) <= value {
        if value.is_multiple_of(p) {
            while value.is_multiple_of(p) {
                value /= p;
            }
            result -= result / p;
        }
        p = if p == 2 { 3 } else { p + 2 };
    }
    if value > 1 {
        result -= result / value;
    }
    Response::ok(TOOL_EULER_TOTIENT)
        .result(result.to_string())
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combination_basic() {
        assert!(combination(5, 2).contains("RESULT: 10"));
        assert!(combination(10, 3).contains("RESULT: 120"));
    }

    #[test]
    fn combination_edge_cases() {
        assert!(combination(0, 0).contains("RESULT: 1"));
        assert!(combination(5, 0).contains("RESULT: 1"));
        assert!(combination(5, 5).contains("RESULT: 1"));
        assert!(combination(5, 7).contains("RESULT: 0"));
    }

    #[test]
    fn combination_large_arbitrary_precision() {
        // C(50, 25) = 126410606437752 (15 digits)
        assert!(combination(50, 25).contains("RESULT: 126410606437752"));
    }

    #[test]
    fn combination_negative_errors() {
        let out = combination(-1, 2);
        assert!(out.starts_with("COMBINATION: ERROR\nREASON: [OUT_OF_RANGE]"));
    }

    #[test]
    fn permutation_basic() {
        // P(5,2) = 5*4 = 20
        assert!(permutation(5, 2).contains("RESULT: 20"));
        // P(10,3) = 10*9*8 = 720
        assert!(permutation(10, 3).contains("RESULT: 720"));
    }

    #[test]
    fn permutation_edge_zero() {
        assert!(permutation(5, 0).contains("RESULT: 1"));
    }

    #[test]
    fn fibonacci_known_values() {
        assert!(fibonacci(0).contains("RESULT: 0"));
        assert!(fibonacci(1).contains("RESULT: 1"));
        assert!(fibonacci(10).contains("RESULT: 55"));
        assert!(fibonacci(20).contains("RESULT: 6765"));
    }

    #[test]
    fn fibonacci_large_arbitrary_precision() {
        // fib(100) = 354224848179261915075
        assert!(fibonacci(100).contains("RESULT: 354224848179261915075"));
    }

    #[test]
    fn fibonacci_negative_errors() {
        let out = fibonacci(-1);
        assert!(out.starts_with("FIBONACCI: ERROR"));
    }

    #[test]
    fn is_prime_small_primes() {
        assert!(is_prime(2).contains("IS_PRIME: true"));
        assert!(is_prime(3).contains("IS_PRIME: true"));
        assert!(is_prime(5).contains("IS_PRIME: true"));
        assert!(is_prime(13).contains("IS_PRIME: true"));
    }

    #[test]
    fn is_prime_composites() {
        assert!(is_prime(0).contains("IS_PRIME: false"));
        assert!(is_prime(1).contains("IS_PRIME: false"));
        assert!(is_prime(4).contains("IS_PRIME: false"));
        assert!(is_prime(15).contains("IS_PRIME: false"));
        assert!(is_prime(100).contains("IS_PRIME: false"));
    }

    #[test]
    fn is_prime_large() {
        // 982451653 is a known large prime.
        assert!(is_prime(982_451_653).contains("IS_PRIME: true"));
    }

    #[test]
    fn is_prime_negative_errors() {
        let out = is_prime(-7);
        assert!(out.starts_with("IS_PRIME: ERROR"));
        assert!(out.contains("non-negative integers"));
    }

    #[test]
    fn next_prime_basic() {
        assert!(next_prime(0).contains("RESULT: 2"));
        assert!(next_prime(2).contains("RESULT: 3"));
        assert!(next_prime(7).contains("RESULT: 11"));
        assert!(next_prime(100).contains("RESULT: 101"));
    }

    #[test]
    fn prime_factors_basic() {
        assert!(prime_factors(12).contains("FACTORS: 2,2,3"));
        assert!(prime_factors(100).contains("FACTORS: 2,2,5,5"));
    }

    #[test]
    fn prime_factors_of_prime() {
        let out = prime_factors(13);
        assert!(out.contains("FACTORS: 13"));
        assert!(out.contains("COUNT: 1"));
    }

    #[test]
    fn prime_factors_of_two() {
        assert!(prime_factors(2).contains("FACTORS: 2"));
    }

    #[test]
    fn prime_factors_one_is_empty_product() {
        // The prime factorization of 1 is the empty product — no primes
        // multiply to give 1. This matches the convention in eulerTotient(1).
        let out = prime_factors(1);
        assert!(out.contains("PRIME_FACTORS: OK"));
        assert!(out.contains("FACTORS: "));
        assert!(out.contains("COUNT: 0"));
    }

    #[test]
    fn prime_factors_zero_errors() {
        let out = prime_factors(0);
        assert!(out.starts_with("PRIME_FACTORS: ERROR"));
    }

    #[test]
    fn euler_totient_known_values() {
        // φ(1) = 1
        assert!(euler_totient(1).contains("RESULT: 1"));
        // φ(9) = 6 (1,2,4,5,7,8)
        assert!(euler_totient(9).contains("RESULT: 6"));
        // φ(10) = 4 (1,3,7,9)
        assert!(euler_totient(10).contains("RESULT: 4"));
        // φ(prime p) = p - 1
        assert!(euler_totient(13).contains("RESULT: 12"));
    }
}
