//! The product: find a pattern of waste in a customer's runs, apply the
//! catalogue's technique for it, check that it fits, and adapt.
//!
//! ```text
//! runs ──> Detector (app port) ──> Finding
//! Finding + Catalogue ──> Adoption: Shown ──> Partial ──> Full ──> Kept
//!                                     │          └──────────┴──> Unfit: put back, try the next
//! fit (against the catalogue's limits) and reading (what is expected, what is seen so far)
//! ```
//!
//! The rules are stated in [the domain's doc](crate::domain#two-layers). This
//! module never uses the Lab: it shares the measuring parts and the guard on
//! writing ([`crate::domain::measure`], [`crate::domain::places`]) and takes
//! what was proven as a [`catalogue::Catalogue`].

pub mod adoption;
pub mod catalogue;
pub mod finding;
pub mod fit;
pub mod portfolio;
