//! Native Confluence context contracts. Content and links cannot supply credentials or routing.
pub mod attachments;
pub mod comments;
pub mod content_state;
mod models;
pub mod pages;
mod reads;
mod registration;
mod selection;
pub use models::*;
pub use reads::{read_get, read_page};
pub use registration::{
    ConfluenceContext, PAGE_GET, PAGE_LIST, PAGE_SEARCH, SPACE_GET, SPACE_LIST, projection,
    response_schema_for,
};
