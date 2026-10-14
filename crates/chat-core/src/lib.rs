//! Dialog client core.
//!
//! Everything that is not UI lives here: the Signal protocol (via libsignal), the local
//! encrypted database, the server API client, message sync and attachment handling. The
//! same crate is used by the desktop app today and by the mobile apps later.

pub mod api;
pub mod content;
pub mod crypto;
pub mod db;
pub mod engine;
pub mod keytrans;
pub mod media_crypto;
pub mod error;
pub mod model;
pub mod profiles;
pub mod revocation;
pub mod rpc;
pub mod sanitize;
pub mod store;
pub mod util;
pub mod voice_mask;

pub use engine::{ConnState, Core, CoreConfig, EventSink, Inner};
pub use error::{CoreError, ErrorCode, Result};
