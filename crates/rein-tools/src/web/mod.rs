// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-21
// Purpose: Web tools module (web_search via Parallel AI / Brave Search, web_fetch via ureq + scraper + htmd)

/// HTTP fetch tool with content-type dispatch and HTML-to-Markdown conversion.
pub mod fetch;
/// Web search tool supporting Parallel AI and Brave Search backends.
pub mod search;

pub use fetch::WebFetchTool;
pub use search::{SearchBackend, WebSearchTool};

/// Maximum number of search results accepted by `web_search`.
pub const MAX_SEARCH_RESULTS: u8 = 20;

/// HTTP client timeout for `web_fetch` and `web_search` (seconds).
pub const FETCH_TIMEOUT_SECS: u64 = 30;

/// Output size cap for `web_fetch` responses (bytes).
pub const FETCH_OUTPUT_CAP_BYTES: usize = 200_000;
