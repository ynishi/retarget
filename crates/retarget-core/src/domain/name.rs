//! Names and small checked numbers that every other module of the domain uses.
//!
//! A value with a rule is a type, so that a wrong value is refused once, at the
//! edge, and never travels further. The names here are the domain's own: an
//! adapter translates a harness's identifiers into them (a model id keeps its
//! shape; anything else is mapped onto the lower-case form below).
//!
//! # Invariants
//!
//! - A key-like name ([`RoleKey`], [`LeverKey`], [`PositionId`], [`ProjectId`],
//!   [`Language`], [`Scope`]) is 1 to 64 characters of `a-z`, `0-9`, `_`, `.`
//!   and `-`.
//! - A [`SessionId`] is not empty, at most 128 characters, and holds no control
//!   character; it is the harness's own id, kept as it is.
//! - A [`ModelId`] is 1 to 128 characters of letters, digits and `_ . : [ ] -`,
//!   so that ids such as `model-x[1m]` are kept unchanged.
//! - [`Ratio`] is finite and strictly between 0 and 1, [`Share`] is finite in
//!   `[0, 1)`, [`Positive`] is finite and above 0.
//!
//! # Errors
//!
//! [`NameError`] names the kind of value and the input that broke its rule; fix
//! the input at the adapter that produced it.

use std::fmt;
use std::str::FromStr;

/// Why a value was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum NameError {
    /// The text does not have the shape the kind of name requires.
    #[error("{0} `{1}` does not have the required shape")]
    BadName(&'static str, String),
    /// The number is outside the range of its kind (or not finite).
    #[error("{0} must be {1}, got {2}")]
    OutOfRange(&'static str, &'static str, f64),
    /// A harness version is not `major.minor.patch`.
    #[error("harness version `{0}` is not major.minor.patch")]
    BadVersion(String),
}

fn is_key(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.' || b == b'-'
        })
}

fn is_session(s: &str) -> bool {
    !s.is_empty() && s.chars().count() <= 128 && !s.chars().any(char::is_control)
}

fn is_model(s: &str) -> bool {
    (1..=128).contains(&s.len())
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'[' | b']' | b'-')
        })
}

macro_rules! name {
    ($(#[$meta:meta])* $name:ident, $kind:literal, $check:path) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            /// The name as it was given.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl FromStr for $name {
            type Err = NameError;
            fn from_str(s: &str) -> Result<Self, NameError> {
                if $check(s) {
                    Ok(Self(s.to_owned()))
                } else {
                    Err(NameError::BadName($kind, s.to_owned()))
                }
            }
        }

        impl TryFrom<&str> for $name {
            type Error = NameError;
            fn try_from(s: &str) -> Result<Self, NameError> {
                s.parse()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

name!(
    /// The harness's id of one session, so that a run can be traced back to it.
    SessionId, "session id", is_session
);
name!(
    /// The project a run belongs to; part of the task class runs are compared in.
    ProjectId, "project id", is_key
);
name!(
    /// The main language of a project; part of the task class.
    Language, "language", is_key
);
name!(
    /// The stable name of a subagent role, so that a lever can be scoped to it.
    RoleKey, "role key", is_key
);
name!(
    /// The identity of a lever, given by an adapter's catalogue, not the
    /// harness's own key, so that the same lever keeps its name across versions.
    LeverKey, "lever key", is_key
);
name!(
    /// One value a lever can take.
    PositionId, "position", is_key
);
name!(
    /// The harness's model id, kept unchanged so that a price table can match it.
    ModelId, "model id", is_model
);
name!(
    /// The scope a ledger belongs to: one experiment at a time is allowed per scope.
    Scope, "scope", is_key
);

/// The version of the harness a run started and ended on. Runs that span a
/// change of version count for neither arm, and a newer version may reopen
/// what was given up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HarnessVersion(u32, u32, u32);

impl HarnessVersion {
    /// A version from its three parts.
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self(major, minor, patch)
    }
}

impl FromStr for HarnessVersion {
    type Err = NameError;
    fn from_str(s: &str) -> Result<Self, NameError> {
        let bad = || NameError::BadVersion(s.to_owned());
        let mut parts = s.split('.');
        let mut next = || -> Result<u32, NameError> {
            parts.next().ok_or_else(bad)?.parse().map_err(|_| bad())
        };
        let version = Self(next()?, next()?, next()?);
        if parts.next().is_some() {
            return Err(bad());
        }
        Ok(version)
    }
}

impl fmt::Display for HarnessVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The id of an experiment. The caller makes it (a UUID v7 as a number), so the
/// domain never draws one itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExperimentId(u128);

impl ExperimentId {
    /// An id from the number the caller made.
    pub fn new(id: u128) -> Self {
        Self(id)
    }
    /// The number.
    pub fn get(self) -> u128 {
        self.0
    }
}

/// The id of a window: the runs assigned with one assignment probability. The
/// ledger gives them out in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(u64);

impl WindowId {
    /// An id from its number.
    pub fn new(id: u64) -> Self {
        Self(id)
    }
    /// The number.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// The digest of what one lever's place in a file holds (the value at its key,
/// or the section it owns), not of the whole file, so that writing one lever
/// never looks like a person editing another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Digest([u8; 32]);

impl Digest {
    /// A digest from its bytes.
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    /// The bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

macro_rules! number {
    ($(#[$meta:meta])* $name:ident, $kind:literal, $range:literal, $ok:expr) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
        pub struct $name(f64);

        impl $name {
            /// The number, refused when it is outside the range of its kind.
            pub fn new(value: f64) -> Result<Self, NameError> {
                let ok: fn(f64) -> bool = $ok;
                if value.is_finite() && ok(value) {
                    Ok(Self(value))
                } else {
                    Err(NameError::OutOfRange($kind, $range, value))
                }
            }
            /// The number.
            pub fn get(self) -> f64 {
                self.0
            }
        }
    };
}

number!(
    /// A proportion strictly between 0 and 1: a threshold, an error rate, a share
    /// of runs assigned to an arm.
    Ratio, "a ratio", "strictly between 0 and 1", |v| v > 0.0 && v < 1.0
);
number!(
    /// A proportion in `[0, 1)`: a margin that may be zero.
    Share, "a share", "at least 0 and below 1", |v| (0.0..1.0).contains(&v)
);
number!(
    /// A number above 0: a tuning constant of a statistic.
    Positive, "a positive number", "above 0", |v| v > 0.0
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_refuse_wrong_shapes() {
        for bad in ["", "Upper", "has space", &"x".repeat(65), "slash/no"] {
            assert!(bad.parse::<LeverKey>().is_err(), "{bad:?} must be refused");
        }
        for good in ["cache.off_main_ttl", "a", "x-1_2.3", &"x".repeat(64)] {
            assert!(
                good.parse::<LeverKey>().is_ok(),
                "{good:?} must be accepted"
            );
        }
    }

    #[test]
    fn model_ids_keep_their_shape() {
        assert!("claude-opus-5-5[1m]".parse::<ModelId>().is_ok());
        assert!("bad model".parse::<ModelId>().is_err());
    }

    #[test]
    fn session_ids_refuse_control_characters_and_empty() {
        assert!("abc-123".parse::<SessionId>().is_ok());
        assert!("".parse::<SessionId>().is_err());
        assert!("a\nb".parse::<SessionId>().is_err());
    }

    #[test]
    fn versions_parse_and_order() {
        let a: HarnessVersion = "2.1.300".parse().unwrap();
        let b: HarnessVersion = "2.1.319".parse().unwrap();
        assert!(a < b);
        assert_eq!(a.to_string(), "2.1.300");
        for bad in ["2.1", "2.1.3.4", "x.1.2", ""] {
            assert!(bad.parse::<HarnessVersion>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn numbers_keep_their_range() {
        assert!(Ratio::new(0.0).is_err());
        assert!(Ratio::new(1.0).is_err());
        assert!(Ratio::new(f64::NAN).is_err());
        assert!(Ratio::new(0.5).is_ok());
        assert!(Share::new(0.0).is_ok());
        assert!(Share::new(1.0).is_err());
        assert!(Positive::new(0.0).is_err());
        assert!(Positive::new(f64::INFINITY).is_err());
    }
}
