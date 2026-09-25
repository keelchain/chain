//! Integer money math. `Amount` is `u128` so 18-decimal assets fit with room
//! to spare; intermediate products go through 256 bits so `qty × price`
//! can never overflow silently.

use primitive_types::U256;

pub type Amount = u128;

/// 10^n as an `Amount`. Panics past 10^38 (no asset has that many decimals).
pub const fn pow10(n: u32) -> Amount {
    let mut out: Amount = 1;
    let mut i = 0;
    while i < n {
        out *= 10;
        i += 1;
    }
    out
}

/// `floor(a × b / d)` with a 256-bit intermediate. `None` only when `d == 0`
/// or the result itself does not fit in 128 bits.
pub fn mul_div_floor(a: Amount, b: Amount, d: Amount) -> Option<Amount> {
    if d == 0 {
        return None;
    }
    let prod = U256::from(a) * U256::from(b);
    let q = prod / U256::from(d);
    if q > U256::from(u128::MAX) {
        None
    } else {
        Some(q.as_u128())
    }
}

/// Quote units a `quantity` of base costs at `price` (floored).
pub fn quote_amount(quantity: Amount, price: Amount, base_decimals: u32) -> Amount {
    mul_div_floor(quantity, price, pow10(base_decimals)).unwrap_or(Amount::MAX)
}

/// Base units `quote` buys at `price` (floored). Zero when price is zero.
pub fn base_for_quote(quote: Amount, price: Amount, base_decimals: u32) -> Amount {
    if price == 0 {
        return 0;
    }
    mul_div_floor(quote, pow10(base_decimals), price).unwrap_or(Amount::MAX)
}

/// The taker fee on what the taker RECEIVES, in that asset (floored).
pub fn fee_of(received: Amount, bps: u32) -> Amount {
    if bps == 0 {
        return 0;
    }
    mul_div_floor(received, bps as Amount, 10_000).unwrap_or(Amount::MAX)
}

/// Round a base quantity down to a whole number of lots.
pub fn floor_to_lot(quantity: Amount, lot_size: Amount) -> Amount {
    if lot_size == 0 {
        return quantity;
    }
    (quantity / lot_size) * lot_size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_amount_floors_at_base_decimals() {
        // 0.015 BTC (1_500_000 sats) at 60,000.00 USDT (60_000_000_000 micro) = 900 USDT.
        assert_eq!(quote_amount(1_500_000, 60_000_000_000, 8), 900_000_000);
        // Sub-unit remainders are dropped, never rounded up.
        assert_eq!(quote_amount(1, 60_000_000_000, 8), 600);
        assert_eq!(quote_amount(1, 3, 8), 0);
    }

    #[test]
    fn base_for_quote_inverts_quote_amount_conservatively() {
        let qty = base_for_quote(900_000_000, 60_000_000_000, 8);
        assert_eq!(qty, 1_500_000);
        assert!(quote_amount(qty, 60_000_000_000, 8) <= 900_000_000);
        assert_eq!(base_for_quote(1, 0, 8), 0);
    }

    #[test]
    fn fees_and_lots() {
        assert_eq!(fee_of(1_000_000, 20), 2000); // 0.2%
        assert_eq!(fee_of(1_000_000, 0), 0);
        assert_eq!(floor_to_lot(1_234_567, 1000), 1_234_000);
        assert_eq!(floor_to_lot(999, 1000), 0);
    }

    #[test]
    fn wide_products_do_not_overflow() {
        // 1e9 ETH in wei (1e27) at a 1e12 quote price: product 1e39 > u128::MAX.
        let wei = pow10(27);
        let price = pow10(12);
        assert_eq!(quote_amount(wei, price, 18), pow10(21));
        assert_eq!(mul_div_floor(u128::MAX, u128::MAX, 1), None);
        assert_eq!(mul_div_floor(7, 3, 0), None);
    }
}
