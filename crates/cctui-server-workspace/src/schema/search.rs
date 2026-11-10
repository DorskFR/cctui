use cctui_server_core::schema::Migration;

pub const SEARCH_PACKAGE: &str = "workspace_search";

pub const SEARCH: &[Migration] = &[Migration {
    version: 1,
    name: "transcript_search",
    sql: include_str!("../../schema/search/0001_transcript_search.sql"),
}];
