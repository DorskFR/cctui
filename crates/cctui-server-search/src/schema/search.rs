use cctui_server_kernel::schema::Migration;

pub const SEARCH_PACKAGE: &str = "workspace_search";

pub const SEARCH: &[Migration] = &[
    Migration {
        version: 1,
        name: "transcript_search",
        sql: include_str!("../../schema/search/0001_transcript_search.sql"),
    },
    Migration {
        version: 2,
        name: "retire_search_text_index",
        sql: include_str!("../../schema/search/0002_retire_search_text_index.sql"),
    },
];
