//! Whether retarget may write a lever's place in a file, or put it back.
//!
//! This is the rule that keeps a person's edit safe, stated once:
//!
//! - What is compared is the digest of the lever's own place (the value at its
//!   key, or the section it owns), never of the whole file, so that writing one
//!   lever never looks like a person editing another.
//! - The comparison is made before every write: before the first write of an
//!   experiment, before each run's position is written, and before a change is
//!   put back. The adapter reads the place, and the core decides.
//! - Retarget expects the place to hold what it last saw there: what it held
//!   when the experiment started, or what retarget last wrote. Anything else is
//!   a person's edit. Then nothing is written: the experiment stops and says
//!   so, and the place stays as the person left it until they resolve it.
//!
//! ```text
//! adapter reads the place ──> digest ──> check_write(expected, current)
//!                                         ├─ Proceed      ──> write, record the new digest
//!                                         └─ ExternalEdit ──> stop; nothing is written
//! ```

use crate::domain::name::Digest;

/// What retarget expects a lever's place to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Baseline {
    /// Nothing: the place did not exist.
    Empty,
    /// These contents.
    Held(Digest),
}

/// Whether a write may go ahead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutBack {
    /// The place holds what retarget expects.
    Proceed,
    /// Someone else changed it: write nothing.
    ExternalEdit,
}

/// Whether the place still holds what retarget expects.
pub fn check_write(expected: &Baseline, current: Option<&Digest>) -> PutBack {
    match (expected, current) {
        (Baseline::Empty, None) => PutBack::Proceed,
        (Baseline::Held(want), Some(have)) if want == have => PutBack::Proceed,
        _ => PutBack::ExternalEdit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_was_expected_lets_a_write_through() {
        let a = Digest::new([1; 32]);
        let b = Digest::new([2; 32]);
        assert_eq!(check_write(&Baseline::Empty, None), PutBack::Proceed);
        assert_eq!(check_write(&Baseline::Held(a), Some(&a)), PutBack::Proceed);
        assert_eq!(
            check_write(&Baseline::Held(a), Some(&b)),
            PutBack::ExternalEdit
        );
        assert_eq!(check_write(&Baseline::Held(a), None), PutBack::ExternalEdit);
        assert_eq!(
            check_write(&Baseline::Empty, Some(&a)),
            PutBack::ExternalEdit
        );
    }
}
