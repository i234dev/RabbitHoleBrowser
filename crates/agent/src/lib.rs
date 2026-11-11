//! The agent interface of RabbitHole: compact semantic snapshots with
//! stable refs, read mode, actions by ref that auto-wait and report what
//! changed, forms as a unit, network log with API hints, events (dialogs,
//! popups, downloads), screenshots with set-of-marks, sessions and
//! isolated contexts. Used by the MCP server and the `rabbithole agent` CLI.
//! See `docs/AI_INTERFACE.md`.

pub mod actions;
pub mod ax;
pub mod browser;
pub mod forms;
pub mod live;
pub mod netinfo;
pub mod overlay;
pub mod read;
pub mod refs;
pub mod shot;
pub mod snapshot;
pub mod summary;
pub mod tools;
pub mod wait;

pub use browser::{Agent, Config, Out};
pub use tools::{call, definitions, parse_line, TOOLS};
