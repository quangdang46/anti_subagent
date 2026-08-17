//! anti-core: pure domain layer — identity model, lifecycle state machine,
//! event schema, config. No I/O beyond what callers inject.

pub mod arbiter;
pub mod capsule;
pub mod config;
pub mod dispatch;
pub mod disposition;
pub mod events;
pub mod loopprev;
pub mod model;
pub mod report;
pub mod routing;
pub mod statemachine;
pub mod work;
