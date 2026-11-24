//! Input construction and provider adapters. Run ownership, permissions and
//! committing a model decision belong to the caller.

mod history;
mod model;
mod notes;

pub use history::*;
pub use model::*;
pub use notes::*;
