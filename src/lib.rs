//! ReHeader Desktop core: the pure, dependency-light rule engine that turns a
//! user's profiles into concrete header operations for a given URL. Kept free
//! of proxy/TLS deps so it compiles and unit-tests quickly.

pub mod proxydetect;
pub mod rules;

pub use rules::{
    AppState, Compiled, Header, HeaderAction, HeaderOp, Plan, Profile, Redirect, Filter,
    default_state, is_valid_header_name,
};
