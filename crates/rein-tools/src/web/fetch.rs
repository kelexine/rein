// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-21
// Purpose: web_fetch tool performing HTTP GET/POST/PUT/DELETE with content-type dispatch and HTML-to-Markdown

use std::time::Duration;

use htmd::HtmlToMarkdown;
use rein_policy::Capability;
use scraper::{ElementRef, Html, Node, Selector};
use serde_json::{Value, json};

use crate::registry::{Tool, ToolError, ToolSchema};
use crate::web::{FETCH_OUTPUT_CAP_BYTES, FETCH_TIMEOUT_SECS};

/// Supported HTTP methods for [`WebFetchTool`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

impl HttpMethod {
    fn parse(raw: Option<&str>) -> Result<Self, ToolError> {
        match raw.map(str::trim).map(str::to_ascii_uppercase).as_deref() {
            None | Some("") | Some("GET") => Ok(Self::Get),
            Some("POST") => Ok(Self::Post),
            Some("PUT") => Ok(Self::Put),
            Some("DELETE") => Ok(Self::Delete),
            Some(other) => Err(ToolError::InvalidArguments {
                tool: "web_fetch".to_owned(),
                reason: format!(
                    "unsupported HTTP method '{other}' (expected GET, POST, PUT, DELETE)"
                ),
            }),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
        }
    }
}

struct FetchArgs {
    url: String,
    method: HttpMethod,
    headers: Vec<(String, String)>,
    body: Option<String>,
}

/// HTTP fetch tool with content-type dispatch (HTML → Markdown, JSON pretty-print, text passthrough).
#[derive(Debug, Default, Clone, Copy)]
pub struct WebFetchTool;

impl WebFetchTool {
    /// Creates a new [`WebFetchTool`].
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

fn parse_fetch_args(arguments: &Value) -> Result<FetchArgs, ToolError> {
    let url = arguments
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::InvalidArguments {
            tool: "web_fetch".to_owned(),
            reason: "missing non-empty string field 'url'".to_owned(),
        })?
        .to_owned();

    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ToolError::InvalidArguments {
            tool: "web_fetch".to_owned(),
            reason: "url must start with http:// or https://".to_owned(),
        });
    }

    let method = HttpMethod::parse(arguments.get("method").and_then(Value::as_str))?;
    let mut headers = Vec::new();
    if let Some(items) = arguments.get("headers").and_then(Value::as_array) {
        for item in items {
            let name = item.get("name").and_then(Value::as_str);
            let value = item.get("value").and_then(Value::as_str);
            match (name, value) {
                (Some(n), Some(v)) if !n.trim().is_empty() => {
                    headers.push((n.trim().to_owned(), v.to_owned()));
                }
                _ => {
                    return Err(ToolError::InvalidArguments {
                        tool: "web_fetch".to_owned(),
                        reason:
                            "each header entry must contain non-empty 'name' and 'value' strings"
                                .to_owned(),
                    });
                }
            }
        }
    }
    let body = arguments
        .get("body")
        .and_then(Value::as_str)
        .map(str::to_owned);

    Ok(FetchArgs {
        url,
        method,
        headers,
        body,
    })
}

impl Tool for WebFetchTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "web_fetch".to_owned(),
            description: "Fetch content from an HTTP/HTTPS URL with optional method, headers, and body. Converts HTML to clean Markdown, pretty-prints JSON, and passes through plain text.".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["url"],
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The HTTP or HTTPS URL to fetch."
                    },
                    "method": {
                        "type": "string",
                        "enum": ["GET", "POST", "PUT", "DELETE"],
                        "description": "HTTP method to use (default: GET)."
                    },
                    "headers": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["name", "value"],
                            "properties": {
                                "name": { "type": "string" },
                                "value": { "type": "string" }
                            }
                        },
                        "description": "Optional HTTP headers as name/value pairs."
                    },
                    "body": {
                        "type": "string",
                        "description": "Optional request body for POST/PUT requests."
                    }
                }
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let parsed = parse_fetch_args(arguments)?;
        Ok(Capability::WebFetch {
            url: parsed.url,
            method: parsed.method.as_str().to_owned(),
        })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let parsed = parse_fetch_args(arguments)?;
        execute_fetch(&parsed)
    }
}

fn execute_fetch(args: &FetchArgs) -> Result<String, ToolError> {
    let cfg = ureq::config::Config::builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(FETCH_TIMEOUT_SECS)))
        .build();
    let agent = ureq::Agent::new_with_config(cfg);

    let resp = match args.method {
        HttpMethod::Get => {
            let mut req = agent.get(&args.url);
            for (k, v) in &args.headers {
                req = req.header(k.as_str(), v.as_str());
            }
            req.call()
        }
        HttpMethod::Delete => {
            let mut req = agent.delete(&args.url);
            for (k, v) in &args.headers {
                req = req.header(k.as_str(), v.as_str());
            }
            req.call()
        }
        HttpMethod::Post => {
            let mut req = agent.post(&args.url);
            for (k, v) in &args.headers {
                req = req.header(k.as_str(), v.as_str());
            }
            match &args.body {
                Some(b) => req.send(b.as_str()),
                None => req.send_empty(),
            }
        }
        HttpMethod::Put => {
            let mut req = agent.put(&args.url);
            for (k, v) in &args.headers {
                req = req.header(k.as_str(), v.as_str());
            }
            match &args.body {
                Some(b) => req.send(b.as_str()),
                None => req.send_empty(),
            }
        }
    }
    .map_err(|err| ToolError::Execution {
        tool: "web_fetch".to_owned(),
        reason: format!("request failed: {err}"),
    })?;

    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/plain")
        .to_ascii_lowercase();
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_owned();

    let raw = resp
        .into_body()
        .read_to_string()
        .map_err(|err| ToolError::Execution {
            tool: "web_fetch".to_owned(),
            reason: format!("failed to read response body: {err}"),
        })?;

    if !(200..300).contains(&status) {
        return Err(ToolError::Execution {
            tool: "web_fetch".to_owned(),
            reason: format!("HTTP {status}: {}", cap_output(raw)),
        });
    }

    let formatted = format_by_mime(&mime, &raw)?;
    Ok(cap_output(formatted))
}

fn format_by_mime(mime: &str, raw: &str) -> Result<String, ToolError> {
    match mime {
        "text/html" | "application/xhtml+xml" => html_to_markdown(raw),
        "application/json" => Ok(pretty_json(raw)),
        t if t.starts_with("text/") => Ok(raw.to_owned()),
        other => Err(ToolError::Execution {
            tool: "web_fetch".to_owned(),
            reason: format!("unsupported content type '{other}'"),
        }),
    }
}

fn cap_output(output: String) -> String {
    if output.len() <= FETCH_OUTPUT_CAP_BYTES {
        return output;
    }
    let boundary = output.floor_char_boundary(FETCH_OUTPUT_CAP_BYTES);
    format!(
        "{}\n\n[Output truncated: {} bytes total, showing first {} bytes]",
        &output[..boundary],
        output.len(),
        boundary
    )
}

/// Converts HTML to clean Markdown by scoping to primary content and stripping noise subtrees.
pub fn html_to_markdown(html: &str) -> Result<String, ToolError> {
    let doc = Html::parse_document(html);
    let noise_tags = [
        "script", "style", "noscript", "head", "meta", "link", "iframe", "object", "embed", "svg",
        "canvas",
    ];
    let content_selectors = [
        "main",
        "article",
        "[role=main]",
        "#content",
        ".content",
        ".post",
        ".entry",
        "body",
    ];

    let content_html = content_selectors
        .iter()
        .filter_map(|sel| Selector::parse(sel).ok())
        .find_map(|selector| doc.select(&selector).next().map(|node| node.html()))
        .unwrap_or_else(|| html.to_owned());

    let frag = Html::parse_fragment(&content_html);
    let mut cleaned = String::with_capacity(content_html.len() / 2);
    collect_clean_html(&frag.root_element(), &noise_tags, &mut cleaned);
    let wrapped = format!("<div>{cleaned}</div>");

    HtmlToMarkdown::new()
        .convert(&wrapped)
        .map_err(|err| ToolError::Execution {
            tool: "web_fetch".to_owned(),
            reason: format!("HTML-to-Markdown conversion failed: {err}"),
        })
}

fn collect_clean_html(node: &ElementRef, noise: &[&str], out: &mut String) {
    for child in node.children() {
        match child.value() {
            Node::Text(t) => out.push_str(t),
            Node::Element(el) => {
                let tag = el.name();
                if noise.contains(&tag) {
                    continue;
                }
                if let Some(el_ref) = ElementRef::wrap(child) {
                    out.push('<');
                    out.push_str(tag);
                    for (k, v) in el.attrs() {
                        if !k.starts_with("on") {
                            out.push_str(&format!(" {k}=\"{v}\""));
                        }
                    }
                    out.push('>');
                    collect_clean_html(&el_ref, noise, out);
                    out.push_str(&format!("</{tag}>"));
                }
            }
            _ => {}
        }
    }
}

fn pretty_json(raw: &str) -> String {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| raw.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_url_scheme_and_method() {
        let tool = WebFetchTool::new();
        assert!(
            tool.required_capability(&json!({"url": "file:///etc/passwd"}))
                .is_err()
        );
        assert!(
            tool.required_capability(&json!({"url": "not-a-url"}))
                .is_err()
        );
        assert!(
            tool.required_capability(&json!({"url": "https://example.com", "method": "PATCH"}))
                .is_err()
        );
        let cap = tool
            .required_capability(&json!({"url": "https://example.com", "method": "post"}))
            .expect("valid fetch args");
        assert_eq!(
            cap,
            Capability::WebFetch {
                url: "https://example.com".to_owned(),
                method: "POST".to_owned(),
            }
        );
    }

    #[test]
    fn html_noise_and_event_handlers_stripped() {
        let html = "<html><head><title>T</title></head><body>\
            <script>alert(1)</script><a onclick=\"evil()\" href=\"https://a.com\">link</a>\
            <p>Main content</p></body></html>";
        let md = html_to_markdown(html).expect("html conversion");
        assert!(!md.contains("alert(1)"));
        assert!(!md.contains("onclick"));
        assert!(!md.contains("evil()"));
        assert!(md.contains("Main content"));
        assert!(md.contains("link"));
    }

    #[test]
    fn mime_dispatch_handles_json_text_and_binary() {
        let json_out = format_by_mime("application/json", r#"{"a":1}"#).expect("json");
        assert!(json_out.contains("\"a\": 1"));
        assert_eq!(
            format_by_mime("text/plain", "hello").expect("text"),
            "hello"
        );
        assert!(format_by_mime("application/octet-stream", "bin").is_err());
    }

    #[test]
    fn cap_output_truncates_on_char_boundary() {
        let big = "é".repeat(FETCH_OUTPUT_CAP_BYTES);
        let capped = cap_output(big);
        assert!(capped.contains("[Output truncated:"));
    }
}
