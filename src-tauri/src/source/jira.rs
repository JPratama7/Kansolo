//! Jira Cloud REST API v3 `SourceProvider` implementation.
//!
//! Moves all Jira-specific logic (HTTP, ADF→Markdown parsing, project
//! discovery) out of the legacy `crate::jira` module and behind the
//! [`SourceProvider`](super::SourceProvider) plugin seam, and ports the
//! `buildJql`/`quoteJql` pure functions from `src/jql.ts` so the JQL builder
//! is owned by the same module that runs the query.

use base64::Engine as _;
use serde::Deserialize;

use super::{RawCard, SourceProvider};

/// Builder parts stored as the `jql_parts` sub-object of a Jira source's
/// config JSON. Mirrors the `JqlParts` interface in `src/jql.ts` — keep the
/// camelCase field names exact so the frontend can round-trip the same blob.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JqlParts {
    pub project: String,
    /// `"current"` | `"specific"` | `"any"`.
    pub assignee_mode: String,
    pub assignee: String,
    /// `"unresolved"` | `"all"` | `"specific"`.
    pub status_mode: String,
    pub statuses: Vec<String>,
    /// `"any"` | `"7d"` | `"30d"` | `"90d"`.
    pub updated_within: String,
    /// `"updated"` | `"priority"` | `"created"`.
    pub order_by: String,
}

/// Quote a JQL string literal: wrap in double quotes, escape inner quotes
/// and backslashes. Empty string returns `""`. Port of `quoteJql` in
/// `src/jql.ts`.
fn quote_jql(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Compose a JQL query from builder parts. Returns an empty string when no
/// clauses apply. Clauses are joined with `AND`; `ORDER BY <field> DESC` is
/// appended last. Port of `buildJql` in `src/jql.ts` — every branch matches.
pub fn build_jql(parts: &JqlParts) -> String {
    let mut clauses: Vec<String> = Vec::new();

    let project = parts.project.trim();
    if !project.is_empty() {
        clauses.push(format!("project = {}", quote_jql(project)));
    }

    if parts.assignee_mode == "current" {
        clauses.push("assignee = currentUser()".to_string());
    } else if parts.assignee_mode == "specific" {
        let assignee = parts.assignee.trim();
        if !assignee.is_empty() {
            clauses.push(format!("assignee = {}", quote_jql(assignee)));
        }
    }
    // "any" → no assignee clause.

    if parts.status_mode == "unresolved" {
        clauses.push("resolution = Unresolved".to_string());
    } else if parts.status_mode == "specific" {
        let statuses: Vec<String> = parts
            .statuses
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !statuses.is_empty() {
            let list = statuses.iter().map(|s| quote_jql(s)).collect::<Vec<_>>().join(", ");
            clauses.push(format!("status IN ({list})"));
        }
    }
    // "all" → no status clause.

    if parts.updated_within != "any" {
        clauses.push(format!("updated >= -{}", parts.updated_within));
    }

    if clauses.is_empty() {
        return String::new();
    }
    let query = clauses.join(" AND ");
    format!("{query} ORDER BY {} DESC", parts.order_by)
}

/// Raw Jira Cloud search response (`GET /rest/api/3/search/jql`).
///
/// `nextPageToken` is present when more pages exist on Jira Cloud; pass it
/// back as the `nextPageToken` query param to fetch the next slice. Older
/// on-prem deployments omit it (single page).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    issues: Vec<Issue>,
    #[serde(default)]
    next_page_token: Option<String>,
}

/// Raw issue entry.
#[derive(Debug, Deserialize)]
struct Issue {
    key: String,
    fields: Fields,
}

/// Raw fields object. Optional members tolerate sparse payloads.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fields {
    summary: String,
    status: Status,
    description: Option<serde_json::Value>,
    priority: Option<Priority>,
}

#[derive(Debug, Deserialize)]
struct Status {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Priority {
    name: Option<String>,
}

/// Pure parsing path: JSON text in, normalized [`RawCard`]s out plus the
/// optional `nextPageToken` cursor. No network needed — kept separate from
/// [`JiraSource::fetch_raw`] so it's trivial to unit-test against fixtures.
fn parse_search_response(body: &str) -> Result<(Vec<RawCard>, Option<String>), String> {
    let response: SearchResponse =
        serde_json::from_str(body).map_err(|e| format!("Failed to parse Jira response: {e}"))?;

    let next_page_token = response.next_page_token.filter(|t| !t.is_empty());
    let cards = response
        .issues
        .into_iter()
        .map(|issue| {
            let description = match issue.fields.description {
                Some(value) => adf_to_text(&value),
                None => String::new(),
            };
            let status_name = issue.fields.status.name.unwrap_or_default();
            RawCard {
                source_ref: issue.key,
                title: issue.fields.summary,
                description,
                status_name,
                priority_name: issue.fields.priority.map(|p| p.name).flatten(),
            }
        })
        .collect();
    Ok((cards, next_page_token))
}

/// Convert a Jira description field to Markdown text.
///
/// Jira Cloud REST API v3 returns descriptions as Atlassian Document Format
/// (ADF) — a JSON tree of `doc`/`paragraph`/`heading`/`list`/`text` nodes.
/// Older or custom fields may return a plain string. This handles both and
/// yields a single Markdown string: headings as `#`, lists as `-`/`1.`, code
/// blocks fenced, inline marks (`**bold**`, `*italic*`, `~~strike~~`) and
/// links as `[text](url)`. Blocks are separated by blank lines.
fn adf_to_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => node_to_md(value, 0),
        // Numbers/bools are not valid ADF; fall back to the JSON literal so
        // nothing is silently dropped.
        other => other.to_string(),
    }
}

/// Render a single ADF node to Markdown at the given list nesting depth.
/// Unknown node types recurse into their `content` so new block types
/// degrade gracefully.
fn node_to_md(node: &serde_json::Value, depth: usize) -> String {
    let ty = node.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let content = node.get("content");
    match ty {
        "text" => text_to_md(node),
        "mention" => format!(
            "@{}",
            node.get("attrs")
                .and_then(|a| a.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
        ),
        "emoji" => node
            .get("attrs")
            .and_then(|a| a.get("shortName"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "inlineCard" | "mediaSingle" => link_to_md(node),
        "paragraph" => inline_join(content),
        "heading" => {
            let level = node
                .get("attrs")
                .and_then(|a| a.get("level"))
                .and_then(|v| v.as_u64())
                .unwrap_or(1)
                .clamp(1, 6) as usize;
            format!("{} {}", "#".repeat(level), inline_join(content))
        }
        "codeBlock" => {
            let lang = node
                .get("attrs")
                .and_then(|a| a.get("language"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let body = content
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|n| n.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            format!("```{lang}\n{body}\n```")
        }
        "blockquote" => {
            let body = blocks_join(content, "\n\n", depth);
            body.lines()
                .map(|l| format!("> {l}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
        "bulletList" => list_to_md(content, false, depth),
        "orderedList" => list_to_md(content, true, depth),
        "listItem" => blocks_join(content, "\n", depth),
        // `doc`, `panel`, `expand`, and any future container: blocks
        // separated by a blank line.
        _ => blocks_join(content, "\n\n", depth),
    }
}

/// Render a `text` node, wrapping it in `**`/`*`/`~~`/`` ` `` based on its
/// `marks` array. Multiple marks nest in a fixed order so output is stable.
fn text_to_md(node: &serde_json::Value) -> String {
    let text = node
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if text.is_empty() {
        return String::new();
    }
    let mut out = text;
    let mut has_code = false;
    let mut has_strong = false;
    let mut has_em = false;
    let mut has_strike = false;
    if let Some(marks) = node.get("marks").and_then(|m| m.as_array()) {
        for mark in marks {
            let mty = mark.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match mty {
                "code" => has_code = true,
                "strong" => has_strong = true,
                "em" => has_em = true,
                "strike" => has_strike = true,
                "link" => {
                    let href = mark
                        .get("attrs")
                        .and_then(|a| a.get("href"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !href.is_empty() {
                        out = format!("[{out}]({href})");
                    }
                }
                _ => {}
            }
        }
    }
    // Nesting order: code is exclusive (no other marks inside), then strong,
    // em, strike outermost. Keeps round-trips stable.
    if has_code {
        format!("`{out}`")
    } else {
        if has_strong {
            out = format!("**{out}**");
        }
        if has_em {
            out = format!("*{out}*");
        }
        if has_strike {
            out = format!("~~{out}~~");
        }
        out
    }
}

/// Render an `inlineCard`/`mediaSingle` node as a Markdown link, using the
/// URL from `attrs.url` (inlineCard) or `attrs.url`/`data.url` for media.
fn link_to_md(node: &serde_json::Value) -> String {
    let attrs = node.get("attrs");
    let url = attrs
        .and_then(|a| a.get("url").and_then(|v| v.as_str()))
        .or_else(|| {
            node.get("data")
                .and_then(|d| d.get("url").and_then(|v| v.as_str()))
        })
        .unwrap_or("");
    if url.is_empty() {
        String::new()
    } else {
        format!("<{url}>")
    }
}

/// Join inline content (text/mention/emoji/link) with single spaces,
/// collapsing runs of whitespace so adjacent formatted nodes don't glue.
fn inline_join(content: Option<&serde_json::Value>) -> String {
    let Some(arr) = content.and_then(|c| c.as_array()) else {
        return String::new();
    };
    let parts: Vec<String> = arr
        .iter()
        .map(|n| node_to_md(n, 0))
        .filter(|s| !s.is_empty())
        .collect();
    parts
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Join block-level children with `sep`, dropping empty blocks.
fn blocks_join(content: Option<&serde_json::Value>, sep: &str, depth: usize) -> String {
    let Some(arr) = content.and_then(|c| c.as_array()) else {
        return String::new();
    };
    let parts: Vec<String> = arr
        .iter()
        .map(|n| node_to_md(n, depth))
        .filter(|s| !s.is_empty())
        .collect();
    parts.join(sep)
}

/// Render a list. Ordered items get `1. `, `2. `; unordered get `- `.
/// Nested lists indent two spaces per level. Continuation lines of a
/// multi-paragraph item are indented to align with the marker text.
fn list_to_md(content: Option<&serde_json::Value>, ordered: bool, depth: usize) -> String {
    let Some(arr) = content.and_then(|c| c.as_array()) else {
        return String::new();
    };
    let indent = "  ".repeat(depth);
    arr.iter()
        .enumerate()
        .map(|(i, item)| {
            let body = node_to_md(item, depth + 1);
            let prefix = if ordered {
                format!("{}. ", i + 1)
            } else {
                "- ".to_string()
            };
            let cont_indent = " ".repeat(indent.len() + prefix.len());
            let mut lines = body.split('\n');
            let first = lines.next().unwrap_or("");
            let rest: String = lines
                .map(|l| format!("{cont_indent}{l}"))
                .collect::<Vec<_>>()
                .join("\n");
            if rest.is_empty() {
                format!("{indent}{prefix}{first}")
            } else {
                format!("{indent}{prefix}{first}\n{rest}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build the `GET /rest/api/3/search/jql` URL. Tolerates a user-pasted
/// scheme and trailing slash on `base_url`; percent-encodes the JQL. When
/// `page_token` is `Some`, appends `&nextPageToken=...` to fetch the next
/// page of results from Jira Cloud's cursor-paginated search endpoint.
fn build_search_url(base_url: &str, jql: &str, page_token: Option<&str>) -> String {
    let trimmed = base_url.trim();
    // Tolerate users who paste a full URL with a scheme; strip it so we don't
    // end up with `https://https://host`. http:// is upgraded to https://.
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let host = without_scheme.strip_suffix('/').unwrap_or(without_scheme);
    let encoded_jql = urlencoding::encode(jql);
    let mut url = format!(
        "https://{host}/rest/api/3/search/jql?jql={encoded_jql}&fields=key,summary,status,description,priority&maxResults=100"
    );
    if let Some(token) = page_token {
        url.push_str("&nextPageToken=");
        url.push_str(&urlencoding::encode(token));
    }
    url
}

/// Build the `GET /rest/api/3/project` URL. Same host normalization as
/// [`build_search_url`]; no JQL to encode.
fn build_projects_url(base_url: &str) -> String {
    let trimmed = base_url.trim();
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let host = without_scheme.strip_suffix('/').unwrap_or(without_scheme);
    format!("https://{host}/rest/api/3/project")
}

/// Build a `Basic <base64(email:token)>` header value. The token never
/// appears in plaintext in the result.
fn basic_auth_header(email: &str, token: &str) -> String {
    let credentials = format!("{email}:{token}");
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(credentials.as_bytes())
    )
}

/// Read a string field from a JSON config object, returning a descriptive
/// error if it's missing or not a string. Centralizes the per-field
/// validation so [`JiraSource::fetch_raw`] stays readable.
fn config_string(config: &serde_json::Value, field: &str) -> Result<String, String> {
    let value = config
        .get(field)
        .ok_or_else(|| format!("Missing `{field}` in Jira source config"))?;
    value
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("`{field}` in Jira source config must be a string"))
}

/// Fetch one search page and parse it into `(cards, next_page_token)`. The
/// full URL (including encoded JQL) is logged to stderr on transport failure
/// but never surfaced to the caller — error messages stay free of the URL so
/// credentials embedded in the query string can't leak through the UI.
async fn fetch_search_page(
    client: &reqwest::Client,
    url: &str,
    authorization: &str,
) -> Result<(Vec<RawCard>, Option<String>), String> {
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::AUTHORIZATION, authorization)
        .send()
        .await
        .map_err(|e| {
            eprintln!("Jira search request to {url} failed: {e}");
            format!("Could not reach the Jira server: {e}")
        })?;

    let status = response.status();
    if !status.is_success() {
        // Deliberately discard the body so credentials or tokens can never leak.
        let hint = match status.as_u16() {
            401 | 403 => "check your credentials and permissions.",
            410 => "the Jira search endpoint was removed — update the app.",
            _ => "see Jira API docs for this status code.",
        };
        return Err(format!("Jira API error {status}: {hint}"));
    }

    let body = response
        .text()
        .await
        .map_err(|_| "Jira responded, but the body could not be read.".to_string())?;

    parse_search_response(&body)
}

/// Jira Cloud source provider. Config comes via the `config` parameter on
/// each method — the struct itself is stateless so a single shared instance
/// can serve every Jira-backed [`SourceInstance`](crate::db::SourceInstance).
pub struct JiraSource;

#[async_trait::async_trait]
impl SourceProvider for JiraSource {
    fn source_type(&self) -> &'static str {
        "jira"
    }

    fn display_label(&self) -> &'static str {
        "Jira"
    }

    async fn fetch_raw(&self, config: &serde_json::Value) -> Result<Vec<RawCard>, String> {
        let base_url = config_string(config, "base_url")?;
        let email = config_string(config, "email")?;
        let token = config_string(config, "token")?;
        let jql_parts = config
            .get("jql_parts")
            .ok_or_else(|| "Missing `jql_parts` in Jira source config".to_string())?;
        let parts: JqlParts = serde_json::from_value(jql_parts.clone())
            .map_err(|e| format!("Invalid `jql_parts` in Jira source config: {e}"))?;
        let jql = build_jql(&parts);
        if jql.is_empty() {
            // No clauses means an unbounded query — refuse rather than pull
            // the entire instance. The frontend should require a project.
            return Err(
                "Jira source config produces an empty JQL — set a project or filter".to_string(),
            );
        }

        let authorization = basic_auth_header(&email, &token);
        let client = reqwest::Client::new();

        // Jira Cloud paginates search via an opaque `nextPageToken` cursor.
        // Loop until a page returns no token, capping at 50 pages so a
        // misbehaving server can't trap us in an endless cursor cycle. The
        // cursor is threaded through `build_search_url` so the URL shape for
        // each page is unit-testable without network.
        const MAX_PAGES: usize = 50;
        let mut all_cards: Vec<RawCard> = Vec::new();
        let mut page_token: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let url = build_search_url(&base_url, &jql, page_token.as_deref());
            let (cards, next_token) =
                fetch_search_page(&client, &url, &authorization).await?;
            all_cards.extend(cards);
            match next_token {
                Some(t) => page_token = Some(t),
                None => break,
            }
        }
        Ok(all_cards)
    }

    async fn fetch_options(
        &self,
        config: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let base_url = config_string(config, "base_url")?;
        let email = config_string(config, "email")?;
        let token = config_string(config, "token")?;

        let url = build_projects_url(&base_url);
        let authorization = basic_auth_header(&email, &token);

        let response = reqwest::Client::new()
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::AUTHORIZATION, authorization)
            .send()
            .await
            .map_err(|e| {
                eprintln!("Jira projects request to {url} failed: {e}");
                format!("Could not reach the Jira server: {e}")
            })?;

        let status = response.status();
        if !status.is_success() {
            let hint = match status.as_u16() {
                401 | 403 => "check your credentials and permissions.",
                410 => "the Jira project endpoint was removed — update the app.",
                _ => "see Jira API docs for this status code.",
            };
            return Err(format!("Jira API error {status}: {hint}"));
        }

        let body = response
            .text()
            .await
            .map_err(|_| "Jira responded, but the body could not be read.".to_string())?;

        // Pass the raw project array through wrapped in `{ "projects": [...] }`
        // so the frontend's option-fetch contract is uniform across providers.
        let projects: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("Failed to parse Jira response: {e}"))?;
        Ok(serde_json::json!({ "projects": projects }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirror of `DEFAULT_JQL_PARTS` in `src/jql.ts`. Tests build variations
    /// on top of this so every omitted field matches the frontend default.
    fn default_parts() -> JqlParts {
        JqlParts {
            project: String::new(),
            assignee_mode: "current".to_string(),
            assignee: String::new(),
            status_mode: "unresolved".to_string(),
            statuses: Vec::new(),
            updated_within: "any".to_string(),
            order_by: "updated".to_string(),
        }
    }

    #[test]
    fn build_jql_default_parts_current_user_and_unresolved() {
        assert_eq!(
            build_jql(&default_parts()),
            "assignee = currentUser() AND resolution = Unresolved ORDER BY updated DESC"
        );
    }

    #[test]
    fn build_jql_project_only() {
        let mut parts = default_parts();
        parts.project = "SCRUM".to_string();
        assert_eq!(
            build_jql(&parts),
            "project = \"SCRUM\" AND assignee = currentUser() AND resolution = Unresolved ORDER BY updated DESC"
        );
    }

    #[test]
    fn build_jql_all_clauses() {
        let parts = JqlParts {
            project: "SCRUM".to_string(),
            assignee_mode: "specific".to_string(),
            assignee: "ada".to_string(),
            status_mode: "specific".to_string(),
            statuses: vec!["To Do".to_string(), "In Progress".to_string()],
            updated_within: "7d".to_string(),
            order_by: "priority".to_string(),
        };
        assert_eq!(
            build_jql(&parts),
            "project = \"SCRUM\" AND assignee = \"ada\" AND status IN (\"To Do\", \"In Progress\") AND updated >= -7d ORDER BY priority DESC"
        );
    }

    #[test]
    fn build_jql_assignee_any_no_assignee_clause() {
        let mut parts = default_parts();
        parts.project = "X".to_string();
        parts.assignee_mode = "any".to_string();
        assert_eq!(
            build_jql(&parts),
            "project = \"X\" AND resolution = Unresolved ORDER BY updated DESC"
        );
    }

    #[test]
    fn build_jql_status_all_no_status_clause() {
        let mut parts = default_parts();
        parts.project = "X".to_string();
        parts.status_mode = "all".to_string();
        assert_eq!(
            build_jql(&parts),
            "project = \"X\" AND assignee = currentUser() ORDER BY updated DESC"
        );
    }

    #[test]
    fn build_jql_specific_assignee_empty_clause_dropped() {
        let mut parts = default_parts();
        parts.project = "X".to_string();
        parts.assignee_mode = "specific".to_string();
        parts.assignee = "   ".to_string();
        assert_eq!(
            build_jql(&parts),
            "project = \"X\" AND resolution = Unresolved ORDER BY updated DESC"
        );
    }

    #[test]
    fn build_jql_specific_status_empty_list_clause_dropped() {
        let mut parts = default_parts();
        parts.project = "X".to_string();
        parts.status_mode = "specific".to_string();
        parts.statuses = vec!["  ".to_string(), String::new()];
        assert_eq!(
            build_jql(&parts),
            "project = \"X\" AND assignee = currentUser() ORDER BY updated DESC"
        );
    }

    #[test]
    fn build_jql_quotes_and_backslashes_escaped() {
        let mut parts = default_parts();
        parts.project = "PROJ\"WEIRD\\X".to_string();
        parts.assignee_mode = "any".to_string();
        parts.status_mode = "all".to_string();
        assert_eq!(
            build_jql(&parts),
            "project = \"PROJ\\\"WEIRD\\\\X\" ORDER BY updated DESC"
        );
    }

    #[test]
    fn build_jql_updated_within_any_no_updated_clause() {
        let mut parts = default_parts();
        parts.project = "X".to_string();
        parts.updated_within = "any".to_string();
        assert!(!build_jql(&parts).contains("updated >="));
    }

    const FIXTURE: &str = r#"{
        "issues": [
            {
                "key": "PROJ-1",
                "fields": {
                    "summary": "First issue",
                    "status": { "name": "In Progress" },
                    "description": {
                        "type": "doc",
                        "version": 1,
                        "content": [
                            {
                                "type": "paragraph",
                                "content": [
                                    { "type": "text", "text": "Hello" }
                                ]
                            }
                        ]
                    },
                    "priority": { "name": "High" },
                    "assignee": { "displayName": "Ada Lovelace" }
                }
            },
            {
                "key": "PROJ-2",
                "fields": {
                    "summary": "Second issue",
                    "status": {},
                    "priority": null,
                    "assignee": null
                }
            }
        ]
    }"#;

    #[test]
    fn parses_fixture_into_normalized_raw_cards() {
        let (cards, next_token) = parse_search_response(FIXTURE).expect("fixture should parse");
        // FIXTURE has no nextPageToken → single page, cursor is None.
        assert_eq!(next_token, None);

        assert_eq!(cards.len(), 2);

        assert_eq!(cards[0].source_ref, "PROJ-1");
        assert_eq!(cards[0].title, "First issue");
        // ADF description is flattened to Markdown — the paragraph's text
        // node is extracted, not the raw JSON.
        assert_eq!(cards[0].description, "Hello");
        assert_eq!(cards[0].status_name, "In Progress");
        assert_eq!(cards[0].priority_name.as_deref(), Some("High"));
        // RawCard carries no assignee — that's resolved downstream — so we
        // only assert the contract fields above.

        // Second issue exercises missing optional fields.
        assert_eq!(cards[1].source_ref, "PROJ-2");
        assert_eq!(cards[1].title, "Second issue");
        assert_eq!(cards[1].description, "");
        assert_eq!(cards[1].status_name, "");
        assert_eq!(cards[1].priority_name, None);

        // Request-shape helpers: bare-host base_url is trimmed and normalized,
        // jql is percent-encoded, and the Basic header never exposes the token.
        let url = build_search_url(
            "  example.atlassian.net/  ",
            "project = DEMO AND status != \"Done\"",
            None,
        );
        assert_eq!(
            url,
            "https://example.atlassian.net/rest/api/3/search/jql?jql=project%20%3D%20DEMO%20AND%20status%20%21%3D%20%22Done%22&fields=key,summary,status,description,priority&maxResults=100"
        );
        // A user who pastes a full URL with a scheme must not produce a
        // double-scheme URL (`https://https://host`).
        assert_eq!(
            build_search_url("https://example.atlassian.net", "project = DEMO", None),
            "https://example.atlassian.net/rest/api/3/search/jql?jql=project%20%3D%20DEMO&fields=key,summary,status,description,priority&maxResults=100"
        );
        assert_eq!(
            build_search_url("http://example.atlassian.net/", "project = DEMO", None),
            "https://example.atlassian.net/rest/api/3/search/jql?jql=project%20%3D%20DEMO&fields=key,summary,status,description,priority&maxResults=100"
        );
        let auth = basic_auth_header("user", "secret");
        assert!(auth.starts_with("Basic "));
        assert!(!auth.contains("secret"));

        // Project-list endpoint: same host normalization, no JQL encoding.
        assert_eq!(
            build_projects_url("  https://example.atlassian.net/  "),
            "https://example.atlassian.net/rest/api/3/project"
        );
    }

    /// `adf_to_text` exercises: headings, paragraphs with inline marks
    /// (bold/italic/strike/code/link), bullet/ordered lists with nesting,
    /// code blocks, blockquotes, mentions, plain-string fallback, and the
    /// unknown-type graceful-degradation path.
    #[test]
    fn adf_to_text_renders_readable_markdown() {
        let adf = serde_json::json!({
            "type": "doc",
            "version": 1,
            "content": [
                {
                    "type": "heading",
                    "attrs": { "level": 2 },
                    "content": [{ "type": "text", "text": "Overview" }]
                },
                {
                    "type": "paragraph",
                    "content": [
                        { "type": "text", "text": "Hello " },
                        { "type": "text", "text": "world",
                          "marks": [{ "type": "strong" }] },
                        { "type": "text", "text": " and ",
                          "marks": [{ "type": "em" }] },
                        { "type": "text", "text": "code",
                          "marks": [{ "type": "code" }] },
                        { "type": "text", "text": " " },
                        { "type": "text", "text": "link",
                          "marks": [{ "type": "link",
                                      "attrs": { "href": "https://x.test" } }] }
                    ]
                },
                {
                    "type": "bulletList",
                    "content": [
                        {
                            "type": "listItem",
                            "content": [
                                { "type": "paragraph",
                                  "content": [{ "type": "text", "text": "first" }] },
                                {
                                    "type": "bulletList",
                                    "content": [
                                        { "type": "listItem",
                                          "content": [{ "type": "paragraph",
                                                        "content": [{ "type": "text", "text": "nested" }] }] }
                                    ]
                                }
                            ]
                        },
                        {
                            "type": "listItem",
                            "content": [
                                { "type": "paragraph",
                                  "content": [
                                      { "type": "text", "text": "second with " },
                                      { "type": "mention",
                                        "attrs": { "text": "ada" } }
                                  ] }
                            ]
                        }
                    ]
                },
                {
                    "type": "orderedList",
                    "content": [
                        {
                            "type": "listItem",
                            "content": [
                                { "type": "paragraph",
                                  "content": [{ "type": "text", "text": "one" }] }
                            ]
                        },
                        {
                            "type": "listItem",
                            "content": [
                                { "type": "paragraph",
                                  "content": [{ "type": "text", "text": "two" }] }
                            ]
                        }
                    ]
                },
                {
                    "type": "codeBlock",
                    "attrs": { "language": "rust" },
                    "content": [{ "type": "text", "text": "let x = 1;\nlet y = 2;" }]
                },
                {
                    "type": "blockquote",
                    "content": [
                        { "type": "paragraph",
                          "content": [{ "type": "text", "text": "quoted" }] }
                    ]
                }
            ]
        });
        let text = adf_to_text(&adf);
        assert_eq!(
            text,
            "## Overview\n\nHello **world** * and * `code` [link](https://x.test)\n\n\
- first\n    - nested\n- second with @ada\n\n\
1. one\n2. two\n\n\
```rust\nlet x = 1;\nlet y = 2;\n```\n\n\
> quoted"
        );

        // Plain-string descriptions (legacy/custom fields) pass through.
        assert_eq!(adf_to_text(&serde_json::json!("legacy text")), "legacy text");
        // Null → empty.
        assert_eq!(adf_to_text(&serde_json::Value::Null), "");
        // Unknown block type with content still recurses.
        let unknown = serde_json::json!({
            "type": "futureBlock",
            "content": [{ "type": "paragraph",
                          "content": [{ "type": "text", "text": "ok" }] }]
        });
        assert_eq!(adf_to_text(&unknown), "ok");
        // Empty paragraph collapses to empty and is dropped by block joining.
        let doc = serde_json::json!({
            "type": "doc",
            "content": [
                { "type": "paragraph", "content": [] },
                { "type": "paragraph",
                  "content": [{ "type": "text", "text": "after" }] }
            ]
        });
        assert_eq!(adf_to_text(&doc), "after");
    }

    /// Jira Cloud signals more pages with an opaque `nextPageToken` cursor.
    /// This synthesizes a two-issue page carrying a token, asserts the parser
    /// surfaces the cursor alongside the cards, and verifies `build_search_url`
    /// threads the cursor into the next page's URL (percent-encoded, since
    /// tokens may contain characters that aren't URL-safe). An empty token
    /// is treated as "no next page" so a misbehaving server can't loop us.
    #[test]
    fn parses_next_page_token_and_builds_cursor_url() {
        let page = r#"{
            "nextPageToken": "abc 123/==",
            "issues": [
                { "key": "PROJ-3", "fields": { "summary": "Third", "status": {} } },
                { "key": "PROJ-4", "fields": { "summary": "Fourth", "status": {} } }
            ]
        }"#;
        let (cards, next_token) =
            parse_search_response(page).expect("next-page fixture should parse");
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].source_ref, "PROJ-3");
        assert_eq!(cards[1].source_ref, "PROJ-4");
        assert_eq!(next_token.as_deref(), Some("abc 123/=="));

        // The cursor is percent-encoded into the next page URL; the JQL and
        // fields stay identical to the first page.
        let next_url = build_search_url(
            "example.atlassian.net",
            "project = DEMO",
            next_token.as_deref(),
        );
        assert_eq!(
            next_url,
            "https://example.atlassian.net/rest/api/3/search/jql?jql=project%20%3D%20DEMO&fields=key,summary,status,description,priority&maxResults=100&nextPageToken=abc%20123%2F%3D%3D"
        );

        // An empty nextPageToken is normalized to None — no extra page fetch.
        let empty_token = r#"{
            "nextPageToken": "",
            "issues": [{ "key": "PROJ-5", "fields": { "summary": "Last", "status": {} } }]
        }"#;
        let (empty_cards, empty_next) =
            parse_search_response(empty_token).expect("empty-token fixture should parse");
        assert_eq!(empty_cards.len(), 1);
        assert_eq!(empty_next, None);
    }
}
