//! anti-daemon: control-plane daemon — owns state/events/wait and the
//! guard's fail-closed socket (plan §13, §26). The daemon is a coordinator,
//! not an executor: peers are independent OS processes.

pub mod ipc;
pub mod store;
pub mod wait;
