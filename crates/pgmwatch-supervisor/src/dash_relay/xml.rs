use std::collections::HashMap;

#[derive(Debug, Clone)]
pub(super) struct XmlNode {
    pub(super) tag: String,
    pub(super) attrs: HashMap<String, String>,
    pub(super) text: String,
    pub(super) children: Vec<XmlNode>,
}

impl XmlNode {
    pub(super) fn find_child(&self, local_name: &str) -> Option<&XmlNode> {
        self.children.iter().find(|c| c.tag == local_name)
    }

    pub(super) fn children_named(&self, local_name: &str) -> Vec<&XmlNode> {
        self.children
            .iter()
            .filter(|c| c.tag == local_name)
            .collect()
    }
}

fn local_tag_name(raw: &str) -> String {
    raw.rsplit_once(':')
        .map_or(raw, |(_, local)| local)
        .to_string()
}

pub(super) fn parse_xml_root(input: &str) -> Option<XmlNode> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut stack: Vec<XmlNode> = Vec::new();
    let mut roots: Vec<XmlNode> = Vec::new();

    while i < len {
        if let Some(rel_lt) = input[i..].find('<') {
            let text_slice = &input[i..i + rel_lt];
            if !text_slice.trim().is_empty()
                && let Some(top) = stack.last_mut()
            {
                top.text.push_str(&unescape_xml(text_slice));
            }
            i += rel_lt;
        } else {
            break;
        }

        if input[i..].starts_with("<!--") {
            if let Some(end) = input[i + 4..].find("-->") {
                i += 4 + end + 3;
                continue;
            }
            break;
        }
        if input[i..].starts_with("<![CDATA[") {
            if let Some(end) = input[i + 9..].find("]]>") {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&input[i + 9..i + 9 + end]);
                }
                i += 9 + end + 3;
                continue;
            }
            break;
        }
        if input[i..].starts_with("<?") {
            if let Some(end) = input[i + 2..].find("?>") {
                i += 2 + end + 2;
                continue;
            }
            break;
        }
        if input[i..].starts_with("<!") {
            if let Some(end) = input[i + 2..].find('>') {
                i += 2 + end + 1;
                continue;
            }
            break;
        }

        let tag_end = find_tag_close(input, i + 1)?;
        let raw_tag_body = input[i + 1..tag_end].trim();
        i = tag_end + 1;

        if raw_tag_body.is_empty() {
            continue;
        }

        if let Some(close_name) = raw_tag_body.strip_prefix('/') {
            let local = local_tag_name(close_name.trim());
            while let Some(popped) = stack.pop() {
                let matches = popped.tag == local;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(popped);
                } else {
                    roots.push(popped);
                }
                if matches {
                    break;
                }
            }
            continue;
        }

        let self_closing = raw_tag_body.ends_with('/');
        let inner = if self_closing {
            raw_tag_body[..raw_tag_body.len() - 1].trim_end()
        } else {
            raw_tag_body
        };

        let (tag_name, attr_str) = split_tag_and_attrs(inner);
        let node = XmlNode {
            tag: local_tag_name(tag_name),
            attrs: parse_xml_attrs(attr_str),
            text: String::new(),
            children: Vec::new(),
        };

        if self_closing {
            if let Some(parent) = stack.last_mut() {
                parent.children.push(node);
            } else {
                roots.push(node);
            }
        } else {
            stack.push(node);
        }
    }

    while let Some(popped) = stack.pop() {
        if let Some(parent) = stack.last_mut() {
            parent.children.push(popped);
        } else {
            roots.push(popped);
        }
    }

    roots.into_iter().next()
}

fn find_tag_close(s: &str, start: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (idx, b) in s.as_bytes()[start..].iter().enumerate() {
        match (quote, *b) {
            (None, b'"') | (None, b'\'') => quote = Some(*b),
            (Some(q), c) if q == c => quote = None,
            (None, b'>') => return Some(start + idx),
            _ => {}
        }
    }
    None
}

fn split_tag_and_attrs(inner: &str) -> (&str, &str) {
    match inner.find(char::is_whitespace) {
        Some(idx) => (&inner[..idx], inner[idx..].trim_start()),
        None => (inner, ""),
    }
}

fn parse_xml_attrs(mut s: &str) -> HashMap<String, String> {
    let mut attrs = HashMap::new();
    loop {
        s = s.trim_start();
        if s.is_empty() {
            break;
        }
        let Some(eq_idx) = s.find('=') else {
            break;
        };
        let key = s[..eq_idx].trim();
        let after_eq = s[eq_idx + 1..].trim_start();
        let Some(first_byte) = after_eq.as_bytes().first().copied() else {
            break;
        };
        if first_byte != b'"' && first_byte != b'\'' {
            s = after_eq.find(char::is_whitespace).map_or("", |w| &after_eq[w..]);
            continue;
        }
        let quote_char = first_byte as char;
        let rest = &after_eq[1..];
        let Some(close_idx) = rest.find(quote_char) else {
            break;
        };
        let raw_val = &rest[..close_idx];
        let local_key = local_tag_name(key);
        attrs.insert(local_key, unescape_xml(raw_val));
        s = &rest[close_idx + 1..];
    }
    attrs
}

fn unescape_xml(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        if let Some(semi) = tail.find(';') {
            let entity = &tail[1..semi];
            let replacement = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                    u32::from_str_radix(&entity[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                }
                _ if entity.starts_with('#') => {
                    entity[1..].parse::<u32>().ok().and_then(char::from_u32)
                }
                _ => None,
            };
            if let Some(ch) = replacement {
                out.push(ch);
                rest = &tail[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &rest[amp + 1..];
    }
    out.push_str(rest);
    out
}

