//! anti-core: pure domain layer — identity model, lifecycle state machine,
//! event schema, config. No I/O beyond what callers inject.

pub mod config;
pub mod events;
pub mod loopprev;
pub mod model;
pub mod statemachine;
pub mod work;
