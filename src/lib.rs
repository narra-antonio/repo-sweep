//! repo-sweep keeps many local git repositories in sync with their remotes and cleans up the
//! branches whose remote was deleted after a merge.

pub mod check;
pub mod config;
pub mod connectivity;
pub mod discovery;
pub mod git;
pub mod remote;
