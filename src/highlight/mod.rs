mod detect;
mod model;
mod service;
mod theme;
mod tree_sitter;

pub use theme::style_for_class;

pub(crate) use model::{HighlightRequest, HighlightedLine, HighlightedText};
pub(crate) use service::highlight_source;
