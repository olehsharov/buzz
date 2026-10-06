//! Agent hosts: user-approved machines running `buzz host` that run managed
//! agents on the owner's behalf. Wire formats live in [`frames`], lifecycle
//! invariants in [`ops`], persistence in [`store`].

pub(crate) mod channel;
pub(crate) mod frames;
pub(crate) mod ops;
pub(crate) mod store;

pub(crate) use ops::HostOps;
