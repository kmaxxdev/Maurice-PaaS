//! Domain model of Maurice: the entities the panel stores and the rules the
//! validator enforces before anything reaches the host.
//!
//! Nothing in this crate touches the filesystem, systemd or Podman. It is the
//! part that must be correct by construction; the rest of the system renders
//! or executes what is decided here.

pub mod model;
pub mod validate;

pub use model::*;
pub use validate::{Finding, Severity, validate_workspace};
