//! anti-daemon: control-plane daemon — owns state/events/wait and the
//! guard's fail-closed socket (plan §13, §26). The daemon is a coordinator,
//! not an executor: peers are independent OS processes.

pub mod agent_manager;
pub mod bus;
pub mod event_bridge;
pub mod handoff;
pub mod ipc;
pub mod loop_service;
pub mod peer_manager;
pub mod recovery;
pub mod report;
pub mod scheduler;
pub mod store;
pub mod wait;
