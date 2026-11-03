//! Exact decimal analysis and checked rescaling for Parquet. This module never
//! converts a decimal to a floating-point value or allocates a big integer.
use std::io;

#[derive(Clone, Copy, Debug)]
pub struct Decimal<'a> {
    mantissa: &'a str,
    negative: bool,
    pub scale: i32,
    pub integer_digits: usize,
}

impl<'a> Decimal<'a> {
    pub fn parse(text: &'a str) -> io::Result<Self> {
        let invalid = || io::Error::new(io::ErrorKind::InvalidData, "Invalid exact decimal");
        let (negative, unsigned) = match text.as_bytes().first() {
            Some(b'-') => (true, &text[1..]),
            Some(b'+') => (false, &text[1..]),
            _ => (false, text),
        };
        let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
            Some((mantissa, exponent)) => {
                (mantissa, exponent.parse::<i32>().map_err(|_| invalid())?)
            }
            None => (unsigned, 0),
        };
        let mut point = false;
        let mut fractional = 0_i32;
        let mut digits = 0_usize;
        let mut significant = 0_usize;
        for byte in mantissa.bytes() {
            if byte == b'.' && !point {
                point = true;
                continue;
            }
            if !byte.is_ascii_digit() {
                return Err(invalid());
            }
            digits += 1;
            if byte != b'0' || significant > 0 {
                significant += 1;
            }
            if point {
                fractional = fractional.checked_add(1).ok_or_else(invalid)?;
            }
        }
        if digits == 0 {
            return Err(invalid());
        }
        let scale = fractional.checked_sub(exponent).ok_or_else(invalid)?;
        let integer_digits = if significant == 0 {
            0
        } else if scale >= 0 {
            significant.saturating_sub(scale as usize)
        } else {
            significant
                .checked_add(scale.unsigned_abs() as usize)
                .ok_or_else(invalid)?
        };
        Ok(Self {
            mantissa,
            negative,
            scale,
            integer_digits,
        })
    }

    /// Rescale exactly. Reject overflow and any discarded non-zero digit.
    pub fn coefficient(self, precision: u32, scale: u32) -> io::Result<i128> {
        let invalid = || {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Decimal does not fit the exact column precision and scale",
            )
        };
        if precision == 0 || precision > 38 || scale > precision {
            return Err(invalid());
        }
        let shift = i64::from(scale) - i64::from(self.scale);
        let mut digits = self.mantissa.bytes().filter(|byte| *byte != b'.');
        let count = digits.clone().count();
        let keep = if shift < 0 {
            let discarded = usize::try_from(-shift).map_err(|_| invalid())?;
            if discarded > count {
                if digits.all(|byte| byte == b'0') {
                    return Ok(0);
                }
                return Err(invalid());
            }
            if !digits
                .clone()
                .skip(count - discarded)
                .all(|byte| byte == b'0')
            {
                return Err(invalid());
            }
            count - discarded
        } else {
            count
        };
        let mut coefficient = 0_i128;
        for byte in digits.take(keep) {
            coefficient = coefficient
                .checked_mul(10)
                .and_then(|value| value.checked_add(i128::from(byte - b'0')))
                .ok_or_else(invalid)?;
        }
        if coefficient != 0 && shift > 0 {
            let power = u32::try_from(shift).map_err(|_| invalid())?;
            coefficient = coefficient
                .checked_mul(10_i128.checked_pow(power).ok_or_else(invalid)?)
                .ok_or_else(invalid)?;
        }
        if coefficient >= 10_i128.pow(precision) {
            return Err(invalid());
        }
        Ok(if self.negative {
            -coefficient
        } else {
            coefficient
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub scale: u32,
    pub integer_digits: usize,
    pub present: bool,
    pub special: bool,
}

impl Stats {
    pub fn observe(&mut self, text: Option<&str>) -> io::Result<()> {
        let Some(text) = text else {
            return Ok(());
        };
        self.present = true;
        if matches!(
            text,
            "NaN" | "Infinity" | "+Infinity" | "-Infinity" | "inf" | "-inf"
        ) {
            self.special = true;
        } else {
            let decimal = Decimal::parse(text)?;
            self.scale = self.scale.max(decimal.scale.max(0) as u32);
            self.integer_digits = self.integer_digits.max(decimal.integer_digits);
        }
        Ok(())
    }
    pub fn inferred(&self) -> Option<(u32, u32)> {
        if !self.present || self.special {
            return None;
        }
        let precision = self.integer_digits.checked_add(self.scale as usize)?.max(1);
        (precision <= 38).then_some((precision as u32, self.scale))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_scales_choose_one_exact_schema_from_all_values() {
        let mut stats = Stats::default();
        stats.observe(Some("9999")).unwrap();
        stats.observe(Some("1.2345")).unwrap();
        assert_eq!(stats.inferred(), Some((8, 4)));
        assert_eq!(
            Decimal::parse("9999").unwrap().coefficient(8, 4).unwrap(),
            99990000
        );
        assert_eq!(
            Decimal::parse("-1.2345")
                .unwrap()
                .coefficient(8, 4)
                .unwrap(),
            -12345
        );
        assert_eq!(
            Decimal::parse("1.234500")
                .unwrap()
                .coefficient(8, 4)
                .unwrap(),
            12345
        );
        assert!(
            Decimal::parse("1.234501")
                .unwrap()
                .coefficient(8, 4)
                .is_err()
        );
    }
    #[test]
    fn exponents_zeros_precision_boundaries_and_specials_stay_exact() {
        assert_eq!(
            Decimal::parse("1.2345e2")
                .unwrap()
                .coefficient(5, 2)
                .unwrap(),
            12345
        );
        assert_eq!(
            Decimal::parse("-0.000000000000000000000000000000000000000000")
                .unwrap()
                .coefficient(1, 0)
                .unwrap(),
            0
        );
        let max = "99999999999999999999999999999999999999";
        assert_eq!(
            Decimal::parse(max)
                .unwrap()
                .coefficient(38, 0)
                .unwrap()
                .to_string(),
            max
        );
        assert!(
            Decimal::parse("100000000000000000000000000000000000000")
                .unwrap()
                .coefficient(38, 0)
                .is_err()
        );
        assert!(
            Decimal::parse("1e1000")
                .unwrap()
                .coefficient(38, 0)
                .is_err()
        );
        assert!(Decimal::parse("1.2.3").is_err());
        let mut stats = Stats::default();
        assert_eq!(stats.inferred(), None);
        stats.observe(Some("0.001")).unwrap();
        assert_eq!(stats.inferred(), Some((3, 3)));
        stats.observe(Some("NaN")).unwrap();
        assert_eq!(stats.inferred(), None);
    }
    #[test]
    fn values_that_fit_alone_can_exceed_a_common_precision() {
        let mut stats = Stats::default();
        stats
            .observe(Some("99999999999999999999999999999999999999"))
            .unwrap();
        assert_eq!(stats.inferred(), Some((38, 0)));
        stats.observe(Some("0.1")).unwrap();
        assert_eq!(stats.inferred(), None);
    }
}
