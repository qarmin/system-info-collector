//! Making [`system_info_probe`] recordings readable: summaries, tables, CSV and JSON, SVG charts,
//! and benchmarks that run programs repeatedly and compare them.

pub mod benchmark;
pub mod export;
#[cfg(feature = "html")]
pub mod html;
pub mod series;
pub mod stats;
pub mod summary;
pub mod svg;
pub mod table;

pub use summary::Summary;
pub use table::{Cell, Table, TableFormat};
