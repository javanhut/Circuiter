//! Four-state logic values: each bit is 0, 1, X (unknown) or Z (undriven), on buses
//! up to 64 bits wide.

use std::fmt;

/// A bus value. Bit `i` is Z if set in `z`, else X if set in `x`, else `bits`'s bit.
/// Bits at and above `width` are always clear.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Value {
    width: u8,
    bits: u64,
    x: u64,
    z: u64,
}

/// One bit's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bit {
    Zero,
    One,
    X,
    Z,
}

pub const MAX_WIDTH: u32 = 64;

fn mask(width: u32) -> u64 {
    if width >= 64 { u64::MAX } else { (1u64 << width) - 1 }
}

impl Value {
    /// A known value, truncated to `width` bits.
    pub fn new(width: u32, bits: u64) -> Value {
        assert!((1..=MAX_WIDTH).contains(&width), "width {width} out of range");
        Value { width: width as u8, bits: bits & mask(width), x: 0, z: 0 }
    }

    /// Every bit unknown: what an unset register or a conflict holds.
    pub fn unknown(width: u32) -> Value {
        Value { width: width as u8, bits: 0, x: mask(width), z: 0 }
    }

    /// Every bit undriven.
    pub fn floating(width: u32) -> Value {
        Value { width: width as u8, bits: 0, x: 0, z: mask(width) }
    }

    pub fn bit(b: bool) -> Value {
        Value::new(1, b as u64)
    }

    pub fn width(self) -> u32 {
        u32::from(self.width)
    }

    /// The value as a number, if every bit is 0 or 1.
    pub fn known(self) -> Option<u64> {
        (self.x | self.z == 0).then_some(self.bits)
    }

    /// The value as a signed number (two's complement), if known.
    pub fn signed(self) -> Option<i64> {
        let v = self.known()?;
        let w = self.width();
        Some(if w < 64 && v >> (w - 1) & 1 == 1 { (v | !mask(w)) as i64 } else { v as i64 })
    }

    pub fn is_known(self) -> bool {
        self.x | self.z == 0
    }

    /// The raw bit planes: (value, unknown, undriven).
    pub fn planes(self) -> (u64, u64, u64) {
        (self.bits, self.x, self.z)
    }

    pub fn from_planes(width: u32, bits: u64, x: u64, z: u64) -> Value {
        let m = mask(width);
        let z = z & m;
        let x = x & m & !z;
        Value { width: width as u8, bits: bits & m & !(x | z), x, z }
    }

    pub fn get(self, i: u32) -> Bit {
        let b = 1u64 << i;
        if self.z & b != 0 {
            Bit::Z
        } else if self.x & b != 0 {
            Bit::X
        } else if self.bits & b != 0 {
            Bit::One
        } else {
            Bit::Zero
        }
    }

    /// Bits `lo .. lo + width`.
    pub fn slice(self, lo: u32, width: u32) -> Value {
        let m = mask(width);
        let shift = |v: u64| if lo >= 64 { 0 } else { v >> lo };
        Value::from_planes(width, shift(self.bits) & m, shift(self.x) & m, shift(self.z) & m)
    }

    /// `self` above `low`: `{self, low}` in Verilog.
    pub fn concat(self, low: Value) -> Value {
        let w = self.width() + low.width();
        assert!(w <= MAX_WIDTH, "concatenation wider than {MAX_WIDTH} bits");
        let lw = low.width();
        let up = |v: u64| if lw >= 64 { 0 } else { v << lw };
        Value::from_planes(w, up(self.bits) | low.bits, up(self.x) | low.x, up(self.z) | low.z)
    }

    /// The same value at another width: zero-extended or truncated.
    pub fn resize(self, width: u32) -> Value {
        Value::from_planes(width, self.bits, self.x, self.z)
    }

    /// Z bits read as X: what a gate input sees on an undriven wire.
    pub fn as_input(self) -> Value {
        Value { x: self.x | self.z, z: 0, ..self }
    }

    /// Two drivers on one net: equal bits agree, Z gives way, anything else is X.
    pub fn resolve(self, other: Value) -> Value {
        let w = self.width().max(other.width());
        let (a, b) = (self.resize(w), other.resize(w));
        let both_z = a.z & b.z;
        let a_only = b.z & !a.z; // b gives way to a
        let b_only = a.z & !b.z;
        let both = !(a.z | b.z) & mask(w);
        let agree = both & !(a.x | b.x) & !(a.bits ^ b.bits);
        let x = (both & !agree) | (a_only & a.x) | (b_only & b.x);
        let bits = (agree & a.bits) | (a_only & a.bits) | (b_only & b.bits);
        Value::from_planes(w, bits, x, both_z)
    }

    /// Bitwise NOT; unknown stays unknown.
    pub fn invert(self) -> Value {
        let v = self.as_input();
        Value::from_planes(self.width(), !v.bits, v.x, 0)
    }

    /// Bitwise AND: a known 0 wins over X.
    pub fn and(self, other: Value) -> Value {
        let (a, b) = (self.as_input(), other.as_input().resize(self.width()));
        let zero = (!a.bits & !a.x) | (!b.bits & !b.x);
        let one = a.bits & b.bits;
        let m = mask(self.width());
        Value::from_planes(self.width(), one, !(zero | one) & m, 0)
    }

    /// Bitwise OR: a known 1 wins over X.
    pub fn or(self, other: Value) -> Value {
        let (a, b) = (self.as_input(), other.as_input().resize(self.width()));
        let one = a.bits | b.bits;
        let zero = (!a.bits & !a.x) & (!b.bits & !b.x);
        let m = mask(self.width());
        Value::from_planes(self.width(), one, !(zero | one) & m, 0)
    }

    pub fn xor(self, other: Value) -> Value {
        let (a, b) = (self.as_input(), other.as_input().resize(self.width()));
        Value::from_planes(self.width(), a.bits ^ b.bits, a.x | b.x, 0)
    }

    /// The value as hex, X and Z digits where a nibble isn't fully known: `0x00ff`,
    /// `0x0X1z`; single bits as `0`, `1`, `X`, `Z`.
    pub fn hex(self) -> String {
        if self.width == 1 {
            return match self.get(0) {
                Bit::Zero => "0",
                Bit::One => "1",
                Bit::X => "X",
                Bit::Z => "Z",
            }
            .to_owned();
        }
        let digits = self.width().div_ceil(4);
        let mut s = String::from("0x");
        for d in (0..digits).rev() {
            let n = self.slice(d * 4, 4.min(self.width() - d * 4));
            s.push(if n.z == mask(n.width()) {
                'z'
            } else if n.x | n.z != 0 {
                'X'
            } else {
                char::from_digit(n.bits as u32, 16).expect("nibble")
            });
        }
        s
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}'{}", self.width, self.hex())
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_state_logic() {
        let x = Value::unknown(1);
        let (zero, one) = (Value::bit(false), Value::bit(true));
        assert_eq!(zero.and(x), zero, "0 AND X is 0");
        assert_eq!(one.or(x), one, "1 OR X is 1");
        assert_eq!(one.and(x), x);
        assert_eq!(Value::floating(1).invert(), x, "an undriven input reads as X");
        assert_eq!(Value::floating(4).resolve(Value::new(4, 5)), Value::new(4, 5));
        assert_eq!(Value::new(1, 0).resolve(Value::new(1, 1)), x, "a short is X");
    }

    #[test]
    fn slices_concatenation_and_hex() {
        let v = Value::new(32, 0x1234_5678);
        assert_eq!(v.slice(4, 8), Value::new(8, 0x67));
        assert_eq!(Value::new(4, 0xA).concat(Value::new(4, 0x5)), Value::new(8, 0xA5));
        assert_eq!(v.hex(), "0x12345678");
        assert_eq!(Value::new(32, 0xFFFF_FFFF).signed(), Some(-1));
        let mixed = Value::from_planes(8, 0x0F, 0x30, 0xC0);
        assert_eq!(mixed.hex(), "0xXf", "a nibble partly unknown shows X");
        assert_eq!(Value::floating(8).hex(), "0xzz");
    }
}
