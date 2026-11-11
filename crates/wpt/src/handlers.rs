//! Native stand-ins for the wptserve Python handlers (`*.py`) that tests
//! in the directories we run use most.

use crate::server::{Config, Request, Response};

/// A response for `req` when a native handler covers its path.
pub fn handle(req: &Request, cfg: &Config) -> Option<Response> {
    let _ = cfg;
    let name = req.path.rsplit('/').next().unwrap_or("");
    if !name.ends_with(".py") {
        return None;
    }
    None
}
