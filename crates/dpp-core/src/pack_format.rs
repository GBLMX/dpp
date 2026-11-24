use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A Minecraft data pack format, e.g. `121.0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackFormat {
    major: u32,
    minor: u32,
}

impl PackFormat {
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }

    pub const fn major(self) -> u32 {
        self.major
    }

    pub const fn minor(self) -> u32 {
        self.minor
    }
}

impl From<u32> for PackFormat {
    fn from(major: u32) -> Self {
        Self::new(major, 0)
    }
}

impl From<(u32, u32)> for PackFormat {
    fn from((major, minor): (u32, u32)) -> Self {
        Self::new(major, minor)
    }
}

impl fmt::Display for PackFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("expected `MAJOR` or `MAJOR.MINOR`")]
pub struct ParsePackFormatError;

impl FromStr for PackFormat {
    type Err = ParsePackFormatError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        if value.is_empty() {
            return Err(ParsePackFormatError);
        }

        let mut parts = value.split('.');
        let major = parts
            .next()
            .ok_or(ParsePackFormatError)?
            .parse()
            .map_err(|_| ParsePackFormatError)?;
        let minor = match parts.next() {
            Some(part) => part.parse().map_err(|_| ParsePackFormatError)?,
            None => 0,
        };

        if parts.next().is_some() {
            return Err(ParsePackFormatError);
        }

        Ok(Self::new(major, minor))
    }
}

impl Serialize for PackFormat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        [self.major, self.minor].serialize(serializer)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PackFormatRepr {
    Single(u32),
    List(Vec<u32>),
}

impl<'de> Deserialize<'de> for PackFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match PackFormatRepr::deserialize(deserializer)? {
            PackFormatRepr::Single(major) => Ok(Self::new(major, 0)),
            PackFormatRepr::List(values) => match values.as_slice() {
                [major] => Ok(Self::new(*major, 0)),
                [major, minor] => Ok(Self::new(*major, *minor)),
                _ => Err(serde::de::Error::custom(
                    "pack format must be an integer or a two-integer array",
                )),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct Wrapper {
        format: PackFormat,
    }

    #[test]
    fn parses_supported_forms() {
        assert_eq!("121".parse(), Ok(PackFormat::new(121, 0)));
        assert_eq!("121.0".parse(), Ok(PackFormat::new(121, 0)));
        assert_eq!("121.1".parse(), Ok(PackFormat::new(121, 1)));
        assert_eq!(" 121.2 ".parse(), Ok(PackFormat::new(121, 2)));
    }

    #[test]
    fn rejects_invalid_forms() {
        for value in ["", "121.", ".1", "121.1.2", "-1", "abc"] {
            assert!(value.parse::<PackFormat>().is_err(), "{value:?}");
        }
    }

    #[test]
    fn toml_round_trip() {
        let text = toml::to_string(&Wrapper {
            format: PackFormat::new(121, 1),
        })
        .unwrap();
        assert_eq!(text.trim(), "format = [121, 1]");

        let parsed: Wrapper = toml::from_str(&text).unwrap();
        assert_eq!(parsed.format, PackFormat::new(121, 1));
    }

    #[test]
    fn toml_accepts_single_integer() {
        let parsed: Wrapper = toml::from_str("format = 121").unwrap();
        assert_eq!(parsed.format, PackFormat::new(121, 0));
    }
}
