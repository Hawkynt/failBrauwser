//! File operations: copy, move, delete, trash. No UI in here; progress and questions go
//! through [`job::JobCtx`].

pub mod copy;
pub mod delete;
pub mod device;
pub mod fastcopy;
pub mod gvfs;
pub mod job;
pub mod names;
pub mod staging;
