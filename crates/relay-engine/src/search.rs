//! Web search (DuckDuckGo, keyless) and page fetching helpers.

use std::time::Duration;

#[derive(Clone, Debug)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Search DuckDuckGo HTML endpoint and return up to `max` results.
pub async fn web_search(query: &str, max: usize) -> String {
    let max = max.clamp(1, 10);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36")
        .build()
        .unwrap_or_default();
    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        urlencoding::encode(query)
    );
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => return format!("error: search request failed: {e}"),
    };
    if !resp.status().is_success() {
        return format!("error: search returned {}", resp.status());
    }
    let body = resp.text().await.unwrap_or_default();
    let results = parse_ddg(&body, max);
    if results.is_empty() {
        return "No results found.".to_owned();
    }
    let mut out = String::new();
    for (i, r) in results.iter().enumerate() {
        out.push_str(&format!(
            "{}. {}\n   {}\n   {}\n\n",
            i + 1,
            r.title,
            r.url,
            r.snippet
        ));
    }
    out
}

fn parse_ddg(html: &str, max: usize) -> Vec<SearchResult> {
    // Minimal parser: find result__a links and result__snippet text.
    let mut results = Vec::new();
    let re_link =
        regex::Regex::new(r#"<a[^>]*class="result__a"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#).ok();
    let re_snip = regex::Regex::new(r#"<a[^>]*class="result__snippet"[^>]*>(.*?)</a>"#).ok();
    let snippets: Vec<String> = re_snip
        .as_ref()
        .map(|r| {
            r.captures_iter(html)
                .map(|c| strip_tags(c.get(1).map(|m| m.as_str()).unwrap_or("")))
                .collect()
        })
        .unwrap_or_default();
    if let Some(re) = re_link {
        for (i, cap) in re.captures_iter(html).enumerate() {
            if i >= max {
                break;
            }
            let raw_url = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            let url = decode_ddg_url(raw_url);
            let title = strip_tags(cap.get(2).map(|m| m.as_str()).unwrap_or(""));
            let snippet = snippets.get(i).cloned().unwrap_or_default();
            if !title.is_empty() {
                results.push(SearchResult {
                    title,
                    url,
                    snippet,
                });
            }
        }
    }
    results
}

fn decode_ddg_url(raw: &str) -> String {
    // DuckDuckGo wraps URLs in /l/?uddg=...
    if let Some(idx) = raw.find("uddg=") {
        let rest = &raw[idx + 5..];
        let end = rest.find('&').unwrap_or(rest.len());
        let encoded = &rest[..end];
        urlencoding::decode(encoded)
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| encoded.to_owned())
    } else {
        raw.to_owned()
    }
}

pub async fn fetch_url(url: &str) -> String {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return "error: only http(s) URLs allowed".to_owned();
    }
    // Block obvious local/private hosts.
    if let Some(host) = url.split("://").nth(1).and_then(|s| s.split('/').next()) {
        let host = host.split(':').next().unwrap_or(host);
        if host == "localhost"
            || host == "127.0.0.1"
            || host.starts_with("192.168.")
            || host.starts_with("10.")
        {
            return "error: local/private URLs are blocked".to_owned();
        }
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36")
        .build()
        .unwrap_or_default();
    let resp = match client.get(url).send().await {
        Ok(r) => r,
        Err(e) => return format!("error: fetch failed: {e}"),
    };
    if !resp.status().is_success() {
        return format!("error: fetch returned {}", resp.status());
    }
    let body = resp.text().await.unwrap_or_default();
    let text = strip_tags(&body);
    let trimmed: String = text.chars().take(8000).collect();
    if trimmed.trim().is_empty() {
        "No readable text content.".to_owned()
    } else {
        trimmed
    }
}

fn strip_tags(html: &str) -> String {
    // Remove script/style blocks then tags.
    let mut s = html.to_owned();
    // Remove script/style content.
    let re_script = regex::Regex::new(r"(?is)<script.*?</script>").unwrap();
    s = re_script.replace_all(&s, " ").to_string();
    let re_style = regex::Regex::new(r"(?is)<style.*?</style>").unwrap();
    s = re_style.replace_all(&s, " ").to_string();
    let re_tag = regex::Regex::new(r"<[^>]+>").unwrap();
    s = re_tag.replace_all(&s, " ").to_string();
    // Collapse whitespace.
    let re_ws = regex::Regex::new(r"\s+").unwrap();
    re_ws.replace_all(&s, " ").trim().to_owned()
}
