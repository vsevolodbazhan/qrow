//! Exact ISO dates and timestamps. Wall-clock values do not use a time zone.
use chrono::{FixedOffset, NaiveDate, NaiveDateTime, TimeZone as _};
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timestamp {
    pub precision: u8,
    pub instant: bool,
}

impl Timestamp {
    pub fn from_type(name: &str) -> io::Result<Option<Self>> {
        let name = name.trim().to_ascii_lowercase();
        if !name.starts_with("timestamp") && name != "timestamptz" {
            return Ok(None);
        }
        let precision = match name.split_once('(') {
            Some((_, suffix)) => suffix
                .split(')')
                .next()
                .unwrap_or_default()
                .parse::<u8>()
                .map_err(|_| invalid("Invalid timestamp precision"))?,
            None => 6,
        };
        if precision > 9 {
            return Err(invalid(
                "Timestamp precision above 9 is not supported. Choose Text column types.",
            ));
        }
        Ok(Some(Self {
            precision,
            instant: name.starts_with("timestamptz")
                || name.contains("with time zone")
                || name.ends_with("local tz")
                || name.contains("with local time zone"),
        }))
    }

    pub fn nanos(self) -> bool {
        self.precision > 6
    }

    pub fn parse(self, text: &str) -> io::Result<i64> {
        let (day, rest) = text
            .split_once([' ', 'T'])
            .ok_or_else(|| invalid("Invalid ISO timestamp"))?;
        let zone_at = rest
            .char_indices()
            .find_map(|(ix, ch)| (matches!(ch, '+' | '-' | 'Z' | ' ') && ix >= 8).then_some(ix))
            .unwrap_or(rest.len());
        let (clock, zone) = rest.split_at(zone_at);
        let digits = clock
            .split_once('.')
            .map_or(0, |(_, fraction)| fraction.len());
        if digits > 9
            || digits > usize::from(self.precision)
                && clock.split_once('.').is_some_and(|(_, fraction)| {
                    fraction.as_bytes()[usize::from(self.precision)..]
                        .iter()
                        .any(|digit| *digit != b'0')
                })
        {
            return Err(invalid(
                "Timestamp would lose precision. Choose Text column types.",
            ));
        }
        let local =
            NaiveDateTime::parse_from_str(&format!("{day} {clock}"), "%Y-%m-%d %H:%M:%S%.f")
                .map_err(|_| invalid("Invalid ISO timestamp. Choose Text column types."))?;
        // Chrono can represent leap seconds, while Parquet stores elapsed time.
        if local.and_utc().timestamp_subsec_nanos() >= 1_000_000_000 {
            return Err(invalid("Leap-second timestamps need Text column types."));
        }
        let utc = if self.instant {
            let zone = zone.trim();
            if zone == "Z" || zone == "UTC" {
                local.and_utc()
            } else if zone.starts_with(['+', '-']) {
                offset(zone)?
                    .from_local_datetime(&local)
                    .single()
                    .ok_or_else(|| invalid("Invalid timestamp offset"))?
                    .to_utc()
            } else {
                let zone: chrono_tz::Tz = zone.parse().map_err(|_| {
                    invalid("Unknown timestamp time zone. Choose Text column types.")
                })?;
                zone.from_local_datetime(&local)
                    .single()
                    .ok_or_else(|| {
                        invalid(
                            "Timestamp is in a time-zone gap or overlap. Choose Text column types.",
                        )
                    })?
                    .to_utc()
            }
        } else {
            if !zone.is_empty() {
                return Err(invalid(
                    "Timestamp has a time zone but the column does not. Choose Text column types.",
                ));
            }
            local.and_utc()
        };
        if self.nanos() {
            utc.timestamp_nanos_opt().ok_or_else(|| {
                invalid("Timestamp exceeds the nanosecond range. Choose Text column types.")
            })
        } else {
            Ok(utc.timestamp_micros())
        }
    }
}

pub fn date(text: &str) -> io::Result<i32> {
    let day = NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .map_err(|_| invalid("Invalid ISO date. Choose Text column types."))?;
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
    i32::try_from(day.signed_duration_since(epoch).num_days())
        .map_err(|_| invalid("Date exceeds the Parquet range. Choose Text column types."))
}

fn offset(text: &str) -> io::Result<FixedOffset> {
    let invalid_offset = || invalid("Invalid timestamp offset");
    let sign = if text.starts_with('-') { -1 } else { 1 };
    let parts: Vec<_> = text[1..].split(':').collect();
    if parts.len() > 3
        || !parts
            .iter()
            .all(|part| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(invalid_offset());
    }
    let hour: i32 = parts[0].parse().map_err(|_| invalid_offset())?;
    let minute: i32 = parts
        .get(1)
        .map_or(Ok(0), |part| part.parse())
        .map_err(|_| invalid_offset())?;
    let second: i32 = parts
        .get(2)
        .map_or(Ok(0), |part| part.parse())
        .map_err(|_| invalid_offset())?;
    if hour > 23 || minute > 59 || second > 59 {
        return Err(invalid_offset());
    }
    FixedOffset::east_opt(sign * (hour * 3600 + minute * 60 + second)).ok_or_else(invalid_offset)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamps_preserve_pre_epoch_fractional_precision_and_meaning() {
        for precision in 0..=9 {
            let timestamp = Timestamp {
                precision,
                instant: false,
            };
            let fraction = "123456789".get(..usize::from(precision)).unwrap();
            let text = format!(
                "1969-12-31 23:59:59{}{}",
                if precision == 0 { "" } else { "." },
                fraction
            );
            let value = timestamp.parse(&text).unwrap();
            let subsec: i64 = if fraction.is_empty() {
                0
            } else {
                fraction.parse().unwrap()
            };
            let unit = if precision > 6 { 9 } else { 6 };
            assert_eq!(
                value,
                -10_i64.pow(unit) + subsec * 10_i64.pow(unit - u32::from(precision))
            );
        }
        assert_eq!(date("1969-12-31").unwrap(), -1);
        let wall = Timestamp {
            precision: 6,
            instant: false,
        };
        let instant = Timestamp {
            instant: true,
            ..wall
        };
        assert!(wall.parse("1970-01-01 00:00:00+07").is_err());
        let hive_instant = Timestamp::from_type("TIMESTAMP LOCAL TZ").unwrap().unwrap();
        assert!(hive_instant.instant);
        assert!(
            Timestamp::from_type("TIMESTAMP WITH LOCAL TIME ZONE")
                .unwrap()
                .unwrap()
                .instant
        );
        assert_eq!(
            hive_instant.parse("1970-01-01 00:00:00+07").unwrap(),
            -25_200_000_000
        );
        assert!(hive_instant.parse("1970-01-01 00:00:00").is_err());
        assert_eq!(
            instant.parse("1970-01-01 00:00:00+07").unwrap(),
            -25_200_000_000
        );
        assert!(wall.parse("1970-01-01 00:00:00.0000001").is_err());
        assert!(wall.parse("1970-01-01 00:00:60").is_err());
        assert!(Timestamp::from_type("timestamp(12) with time zone").is_err());
    }
    #[test]
    fn named_zones_reject_gaps_and_overlaps() {
        let instant = Timestamp {
            precision: 9,
            instant: true,
        };
        assert_eq!(
            instant.parse("1970-01-01 01:00:00 Europe/Paris").unwrap(),
            0
        );
        assert!(
            instant
                .parse("2026-03-08 02:30:00 America/New_York")
                .is_err()
        );
        assert!(
            instant
                .parse("2026-11-01 01:30:00 America/New_York")
                .is_err()
        );
        assert!(instant.parse("2026-10-09 01:00:00 Unknown/Zone").is_err());
        assert!(instant.parse("1600-01-01 00:00:00 UTC").is_err());
        assert!(instant.parse("2026-10-09 01:00:00+99").is_err());
    }
}
