//! Shared logical values for formats that distinguish numbers, bytes, and text.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{collections::HashSet, io};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Boolean,
    Integer,
    Float,
    Decimal,
    Binary,
    HexBinary,
    Bytea,
    Nested,
    Text,
}

impl Kind {
    pub fn of(name: &str) -> Self {
        let name = name.trim().to_ascii_lowercase();
        let base = name
            .split(['(', '<', '['])
            .next()
            .unwrap_or_default()
            .trim();
        match base {
            "boolean" | "bool" => Self::Boolean,
            "tinyint" | "smallint" | "int" | "integer" | "bigint" | "int2" | "int4" | "int8" => {
                Self::Integer
            }
            "float" | "real" | "double" | "double precision" | "float4" | "float8" => Self::Float,
            "decimal" | "numeric" | "number" => Self::Decimal,
            "binary" => Self::HexBinary,
            "varbinary" => Self::Binary,
            "bytea" => Self::Bytea,
            "array" | "map" | "struct" | "row" | "json" | "jsonb" => Self::Nested,
            _ if name.ends_with("[]") => Self::Nested,
            _ => Self::Text,
        }
    }
}

pub enum Value<'a> {
    Null,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    Decimal(&'a str),
    Text(&'a str),
    Bytes(Vec<u8>),
    Nested(&'a serde_json::value::RawValue),
}

pub fn normalize<'a>(kind: Kind, text: Option<&'a str>) -> io::Result<Value<'a>> {
    let Some(text) = text else {
        return Ok(Value::Null);
    };
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid value for the column type",
        )
    };
    Ok(match kind {
        Kind::Boolean => Value::Boolean(match text {
            "true" | "TRUE" | "t" | "1" => true,
            "false" | "FALSE" | "f" | "0" => false,
            _ => return Err(invalid()),
        }),
        Kind::Integer => Value::Integer(text.parse().map_err(|_| invalid())?),
        Kind::Float => Value::Float(text.parse().map_err(|_| invalid())?),
        Kind::Decimal => Value::Decimal(text),
        Kind::Binary => Value::Bytes(STANDARD.decode(text).map_err(|_| invalid())?),
        Kind::HexBinary => Value::Bytes(hex(text.strip_prefix("0x").ok_or_else(invalid)?)?),
        Kind::Bytea => Value::Bytes(bytea(text)?),
        Kind::Nested => match serde_json::from_str::<&serde_json::value::RawValue>(text) {
            Ok(value) => Value::Nested(value),
            Err(_) => Value::Text(text),
        },
        Kind::Text => Value::Text(text),
    })
}

pub fn hex(text: &str) -> io::Result<Vec<u8>> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid hexadecimal binary value",
        )
    };
    if !text.len().is_multiple_of(2) {
        return Err(invalid());
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16).ok_or_else(invalid)?;
            let low = (pair[1] as char).to_digit(16).ok_or_else(invalid)?;
            Ok((high * 16 + low) as u8)
        })
        .collect()
}

fn bytea(text: &str) -> io::Result<Vec<u8>> {
    if let Some(text) = text.strip_prefix("\\x") {
        return hex(text);
    }
    let mut bytes = Vec::with_capacity(text.len());
    let mut input = text.bytes();
    while let Some(byte) = input.next() {
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        match input.next() {
            Some(b'\\') => bytes.push(b'\\'),
            Some(first @ b'0'..=b'3') => {
                let second = input.next().filter(|b| (b'0'..=b'7').contains(b));
                let third = input.next().filter(|b| (b'0'..=b'7').contains(b));
                if let (Some(second), Some(third)) = (second, third) {
                    bytes.push((first - b'0') * 64 + (second - b'0') * 8 + (third - b'0'));
                } else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Invalid bytea escape",
                    ));
                }
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid bytea escape",
                ));
            }
        }
    }
    Ok(bytes)
}

/// Keep existing unique names and suffix repeated names without collisions.
pub fn column_names<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let names: Vec<_> = names.collect();
    let reserved: HashSet<_> = names.iter().copied().collect();
    let mut used = HashSet::new();
    names
        .into_iter()
        .map(|name| {
            if used.insert(name.to_owned()) {
                return name.to_owned();
            }
            for suffix in 2.. {
                let candidate = format!("{name}_{suffix}");
                if !reserved.contains(candidate.as_str()) && used.insert(candidate.clone()) {
                    return candidate;
                }
            }
            unreachable!()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binary_encodings_from_each_connector_are_equal() {
        for (kind, text) in [
            (Kind::HexBinary, "0x005cff"),
            (Kind::Binary, "AFz/"),
            (Kind::Bytea, "\\x005cff"),
            (Kind::Bytea, "\\000\\\\\\377"),
        ] {
            let Value::Bytes(bytes) = normalize(kind, Some(text)).unwrap() else {
                panic!("bytes");
            };
            assert_eq!(bytes, [0, 92, 255]);
        }
        assert!(normalize(Kind::Bytea, Some("\\400")).is_err());
        assert!(normalize(Kind::HexBinary, Some("0x1")).is_err());
    }
    #[test]
    fn duplicate_names_do_not_overwrite_real_suffix_names() {
        assert_eq!(
            column_names(["id", "id", "id_2", "id"].into_iter()),
            ["id", "id_3", "id_2", "id_4"]
        );
    }

    #[test]
    fn nested_json_borrows_wide_values_without_allocating_a_tree() {
        let text = format!(
            "[{}]",
            "[true,null,12345678901234567890],"
                .repeat(100_000)
                .trim_end_matches(',')
        );
        let Value::Nested(value) = normalize(Kind::Nested, Some(&text)).unwrap() else {
            panic!("valid nested JSON");
        };
        assert!(std::ptr::eq(value.get().as_ptr(), text.as_ptr()));
        assert_eq!(value.get(), text);
        assert!(matches!(
            normalize(Kind::Nested, Some("{broken}")).unwrap(),
            Value::Text("{broken}")
        ));
    }
}
