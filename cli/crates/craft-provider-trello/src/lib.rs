//! Trello Cloud reads for agents doing development work: boards, lists, cards and their
//! comments, labels, checklists, attachments and members. Content never supplies routing or
//! credentials, and every call targets the fixed Trello API host chosen by the trusted transport.
mod models;
mod reads;
mod registration;
mod table;

pub use models::*;
pub use reads::{read_get, read_page};
pub use registration::{TrelloContext, projection, response_schema_for, validate_target};
pub use table::{Kind, commands, is_get, kind_of};
