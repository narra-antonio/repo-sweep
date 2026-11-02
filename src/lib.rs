//! repo-sweep keeps many local git repositories in sync with their remotes and cleans up the
//! branches whose remote was deleted after a merge.

pub mod check;
pub mod config;
pub mod connectivity;
pub mod detect;
pub mod discovery;
pub mod git;
pub mod parallel;
pub mod remote;
pub mod remote_error;
pub mod report;
pub mod sync;
