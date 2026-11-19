//! Bitbucket Cloud reads for agents doing development work: repositories, branches, commits,
//! pull requests with their comments and diffstat, and pipelines. Content never supplies routing
//! or credentials, and every call targets the fixed Bitbucket API host chosen by the trusted
//! transport. The workspace comes only from the bound target.
mod models;
mod reads;
mod registration;
mod table;

pub use models::*;
pub use reads::{read_get, read_page};
pub use registration::{BitbucketContext, projection, response_schema_for, validate_target};
pub use table::{Kind, commands, is_get, kind_of};
