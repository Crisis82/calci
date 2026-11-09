//! Jekyll-style `---` front matter.

use super::*;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct FrontMatter {
    pub title: Option<String>,
}

pub(super) fn extract_front_matter(markdown: &str) -> (Option<FrontMatter>, String) {
    let mut lines = markdown.lines();
    let Some(first) = lines.next() else {
        return (None, markdown.to_string());
    };
    if first.trim() != "---" {
        return (None, markdown.to_string());
    }
    let mut fm_buf = String::new();
    let mut consumed = first.len() + 1;
    let mut closed = false;
    for line in markdown[first.len() + 1..].lines() {
        consumed += line.len() + 1;
        if line.trim() == "---" {
            closed = true;
            break;
        }
        fm_buf.push_str(line);
        fm_buf.push('\n');
    }
    if !closed {
        return (None, markdown.to_string());
    }

    let mut fm = FrontMatter::default();
    for raw in fm_buf.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim().trim_matches('"').trim_matches('\'').to_string();
            if key.as_str() == "title" {
                fm.title = Some(val)
            }
        }
    }

    let body = markdown.get(consumed..).unwrap_or("").to_string();
    (Some(fm), body)
}
