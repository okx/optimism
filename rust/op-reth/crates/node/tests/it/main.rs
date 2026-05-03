#![allow(missing_docs)]
// Adding the second xlayer_8130 test (which threads a closure-returned Box::pin
// future through `node.advance(...)` and then composes inject_tx + new_payload in
// the same scope) widened the test crate's `Unpin` auto-trait walk past rustc 1.94's
// default 128. Bump to match the lib-side bump rationale documented in
// `crates/node/src/lib.rs`.
#![recursion_limit = "1024"]

mod builder;

mod priority;

mod rpc;

mod custom_genesis;

mod p2p_version;
mod xlayer_8130;

const fn main() {}
