//! Binary encoding conventions for all data stored in d-lmdb.
//!
//! This module defines the on-disk wire format: how keys are namespaced
//! and how values are tagged.  Everything that touches raw LMDB bytes
//! must go through the helpers here — no ad-hoc byte manipulation elsewhere.

mod values;

pub(crate) use values::Value;
