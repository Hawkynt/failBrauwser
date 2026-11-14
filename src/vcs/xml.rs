//! Just enough XML to read what Subversion's `--xml` answers: elements by name, their attributes
//! and their text, entities decoded. The client's output is well-formed and flat enough that no
//! general parser is needed — and none is pulled in for it.

/// One element: its attributes as written in the start tag, and what lies between its tags.
#[derive(Debug, Clone, Copy)]
pub struct Element<'a> {
    pub start: &'a str,
    pub body: &'a str,
}

impl<'a> Element<'a> {
    /// An attribute's decoded value.
    pub fn attr(&self, name: &str) -> Option<String> {
        let mut rest = self.start;
        while let Some(at) = rest.find(name) {
            let before_ok = at == 0 || rest[..at].ends_with(|c: char| c.is_whitespace());
            let after = rest[at + name.len()..].trim_start();
            if before_ok && let Some(value) = after.strip_prefix('=') {
                let value = value.trim_start();
                let quote = value.chars().next()?;
                let inner = &value[1..];
                let end = inner.find(quote)?;
                return Some(decode(&inner[..end]));
            }
            rest = &rest[at + name.len()..];
        }
        None
    }

    /// The decoded text of the first child element `name`.
    pub fn child_text(&self, name: &str) -> Option<String> {
        elements(self.body, name).next().map(|e| decode(e.body.trim_matches(|c| c == '\n' || c == '\r')))
    }

    /// The child elements named `name`.
    pub fn children(&self, name: &'a str) -> impl Iterator<Item = Element<'a>> + 'a {
        elements(self.body, name)
    }
}

/// Every element named `name` in `text`, in order (not nested in one another).
pub fn elements<'a>(text: &'a str, name: &'a str) -> impl Iterator<Item = Element<'a>> + 'a {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let mut rest = text;
    std::iter::from_fn(move || {
        loop {
            let at = rest.find(&open)?;
            let after = &rest[at + open.len()..];
            // `<entry` must not match `<entryfoo`.
            if !after.starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/') {
                rest = after;
                continue;
            }
            let tag_end = after.find('>')?;
            let start = &after[..tag_end];
            if start.ends_with('/') {
                rest = &after[tag_end + 1..];
                return Some(Element { start: start.trim_end_matches('/'), body: "" });
            }
            let inner = &after[tag_end + 1..];
            let end = inner.find(&close)?;
            rest = &inner[end + close.len()..];
            return Some(Element { start, body: &inner[..end] });
        }
    })
}

/// The five predefined entities and numeric references.
pub fn decode(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some(semi) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let entity = &tail[1..semi];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity.strip_prefix("#x").and_then(|h| u32::from_str_radix(h, 16).ok()).or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok())).and_then(char::from_u32),
        };
        match ch {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=semi]),
        }
        rest = &tail[semi + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elements_attributes_and_text_are_read() {
        let x = r#"<log><logentry
   revision="3"><author>fix &amp; ture</author><msg>a &lt;b&gt;</msg></logentry><logentry revision="2"/></log>"#;
        let all: Vec<_> = elements(x, "logentry").collect();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].attr("revision").as_deref(), Some("3"));
        assert_eq!(all[0].child_text("author").as_deref(), Some("fix & ture"));
        assert_eq!(all[0].child_text("msg").as_deref(), Some("a <b>"));
        assert_eq!(all[1].attr("revision").as_deref(), Some("2"));
    }

    #[test]
    fn a_longer_name_is_not_taken_for_a_shorter_one() {
        let x = r#"<entryx a="1"></entryx><entry path="p"></entry>"#;
        let e: Vec<_> = elements(x, "entry").collect();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].attr("path").as_deref(), Some("p"));
    }

    #[test]
    fn an_attribute_name_inside_another_is_not_matched() {
        let x = r#"<wc-status props="none" item="modified" revision="2">"#;
        let e = Element { start: &x[10..x.len() - 1], body: "" };
        assert_eq!(e.attr("item").as_deref(), Some("modified"));
        assert_eq!(e.attr("revision").as_deref(), Some("2"));
    }

    #[test]
    fn numeric_references_decode() {
        assert_eq!(decode("&#65;&#x42;&unknown;"), "AB&unknown;");
    }
}
