//! CLI composition shared by the executable and deterministic development contract export.

pub mod args;
pub mod attachment_delete;
pub mod attachment_download;
pub mod attachment_reads;
pub mod auth;
pub mod catalog;
pub mod comment_writes;
pub mod confluence_attachment_download;
pub mod confluence_attachment_upload;
pub mod confluence_pages;
pub mod confluence_reads;
pub mod contracts;
pub mod dispatch;
pub mod field_reads;
pub mod issue_comments;
pub mod issue_context;
pub mod issue_create;
pub mod issue_links;
pub mod issue_mutation;
pub mod issue_reads;
pub mod issue_recovery;
pub mod job_wait;
pub mod operation_preview;
pub mod operations;
pub mod output;
pub mod permission_reads;
pub mod planning;
pub mod planning_publication;
pub mod planning_read;
pub mod planning_review;
pub mod policies;
pub mod profiles;
pub mod project_context;
pub mod project_list;
pub mod provision;
pub mod provision_static;
mod public_commands;
pub mod review_policy;
pub mod saved_requests;
pub mod scans;
pub mod trello_reads;

pub mod local_artifacts;

pub mod attachment_upload;
pub mod whiteboard_scene;
