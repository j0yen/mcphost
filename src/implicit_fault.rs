//! PRD-mcphost-implicit-signup-mints-only-on-success R2: test fault hooks for
//! the rollback of an implicit signup. A test arms a fault for one client name
//! (the display name its implicit tenant gets); the dispatch of that tenant's
//! first call then fails, hangs past the call deadline, or panics. An empty map
//! (production) costs one lock and a miss.

use crate::errors::AppError;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// How the injected dispatch fails.
#[derive(Clone, Copy, Debug)]
pub enum Fault {
    /// The handler returns an error.
    Error,
    /// The handler never returns (the call deadline fires).
    Hang,
    /// The handler panics.
    Panic,
}

fn armed() -> &'static Mutex<HashMap<String, Fault>> {
    static ARMED: OnceLock<Mutex<HashMap<String, Fault>>> = OnceLock::new();
    ARMED.get_or_init(Mutex::default)
}

/// Arms `fault` for the next implicit signup whose tenant is named `client_name`.
pub fn arm(client_name: &str, fault: Fault) {
    if let Ok(mut map) = armed().lock() {
        map.insert(client_name.to_string(), fault);
    }
}

/// Takes (and disarms) the fault armed for `client_name`.
pub(crate) fn take(client_name: &str) -> Option<Fault> {
    armed().lock().ok()?.remove(client_name)
}

/// Plays `fault`: returns the handler error, or never returns, or panics.
pub(crate) async fn fire(fault: Fault) -> AppError {
    match fault {
        Fault::Error => AppError::InvalidArgs("injected handler failure".to_string()),
        Fault::Hang => std::future::pending().await,
        Fault::Panic => panic!("injected handler panic"),
    }
}
