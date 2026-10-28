//! Foundation layer (L0). Depends on nothing else in the workspace.

pub mod clock;
pub mod limits;

pub use clock::{Clock, ManualClock, SystemClock};
pub use limits::Limits;
