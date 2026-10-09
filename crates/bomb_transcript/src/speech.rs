//! A reply as it should be read aloud: Markdown turned into plain sentences.
//!
//! Code blocks become "Code block omitted.", Markdown syntax goes, link text stays (bare links
//! become "link"), list items and headings read as sentences, and tables read row by row. Both
//! apps speak the same sentences, and the player maps each to its time in the audio.

/// The longest a sentence gets before it's split at a comma or space, so rendering starts quickly.
const MAX_SENTENCE: usize = 320;

pub fn speakable_sentences(markdown: &str) -> Vec<String> {
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut fence: Option<String> = None;
    let flush = |current: &mut String, paragraphs: &mut Vec<String>| {
        let text = current.trim();
        if !text.is_empty() {
            paragraphs.push(text.to_string());
        }
        current.clear();
    };
    for raw in markdown.lines() {
        let line = raw.trim();
        if let Some(open) = &fence {
            if line.starts_with(open.as_str()) {
                fence = None;
            }
            continue;
        }
        if line.starts_with("```") || line.starts_with("~~~") {
            fence = Some(line[..3].to_string());
            flush(&mut current, &mut paragraphs);
            paragraphs.push("Code block omitted.".into());
            continue;
        }
        if line.is_empty() || is_rule(line) {
            flush(&mut current, &mut paragraphs);
            continue;
        }
        // Table rows read as their cells; the separator row is skipped.
        if line.starts_with('|') {
            if line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
                continue;
            }
            flush(&mut current, &mut paragraphs);
            let cells: Vec<String> = line.trim_matches('|').split('|').map(|c| inline(c.trim())).filter(|c| !c.is_empty()).collect();
            paragraphs.push(end_sentence(&cells.join(", ")));
            continue;
        }
        let (block, text) = strip_block(line);
        if block {
            // Headings, list items and quotes each read as their own sentence.
            flush(&mut current, &mut paragraphs);
            let spoken = inline(text);
            if !spoken.is_empty() {
                paragraphs.push(end_sentence(&spoken));
            }
            continue;
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&inline(text));
    }
    flush(&mut current, &mut paragraphs);
    paragraphs.iter().flat_map(|p| split_sentences(p)).collect()
}

fn is_rule(line: &str) -> bool {
    line.len() >= 3 && (line.chars().all(|c| c == '-' || c == ' ') || line.chars().all(|c| c == '*' || c == ' ') || line.chars().all(|c| c == '_' || c == ' '))
}

/// Heading, list item or quote markers off the front; true when the line was one of those.
fn strip_block(line: &str) -> (bool, &str) {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        return (true, line[hashes..].trim());
    }
    for marker in ["- [ ] ", "- [x] ", "- [X] ", "- ", "* ", "+ ", "> "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return (true, rest.trim());
        }
    }
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && digits <= 3 {
        let rest = &line[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return (true, rest.trim());
        }
    }
    (false, line)
}

/// Inline Markdown to plain words: links keep their text, images their alt text, emphasis and
/// code marks go, HTML tags go, bare URLs read as "link".
fn inline(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // [text](url) and ![alt](url)
        if c == '[' || (c == '!' && chars.get(i + 1) == Some(&'[')) {
            let start = if c == '!' { i + 2 } else { i + 1 };
            if let Some(close) = (start..chars.len()).find(|&j| chars[j] == ']') {
                if chars.get(close + 1) == Some(&'(') {
                    if let Some(end) = (close + 2..chars.len()).find(|&j| chars[j] == ')') {
                        let label: String = chars[start..close].iter().collect();
                        out.push_str(&inline(&label));
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        // <tags>
        if c == '<' {
            if let Some(end) = (i + 1..chars.len().min(i + 200)).find(|&j| chars[j] == '>') {
                let inside: String = chars[i + 1..end].iter().collect();
                if inside.starts_with("http") {
                    out.push_str("link");
                    i = end + 1;
                    continue;
                }
                if inside.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '/') {
                    i = end + 1;
                    continue;
                }
            }
        }
        // Bare URLs
        if (c == 'h') && (text_at(&chars, i, "https://") || text_at(&chars, i, "http://")) {
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            out.push_str("link");
            continue;
        }
        // Emphasis and code marks: ** __ ~~ ` and a single * or _ hugging a word.
        if c == '`' || c == '~' && chars.get(i + 1) == Some(&'~') {
            i += if c == '~' { 2 } else { 1 };
            continue;
        }
        if c == '*' || c == '_' {
            let doubled = chars.get(i + 1) == Some(&c);
            let prev_word = i > 0 && chars[i - 1].is_alphanumeric();
            let next_word = chars.get(if doubled { i + 2 } else { i + 1 }).is_some_and(|n| n.is_alphanumeric());
            // snake_case and 2*3 keep theirs.
            if !(prev_word && next_word) {
                i += if doubled { 2 } else { 1 };
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn text_at(chars: &[char], at: usize, word: &str) -> bool {
    word.chars().enumerate().all(|(k, w)| chars.get(at + k) == Some(&w))
}

fn end_sentence(text: &str) -> String {
    let text = text.trim();
    if text.ends_with(['.', '!', '?', ':', ';']) { text.to_string() } else { format!("{text}.") }
}

/// A paragraph into sentences, keeping each short enough to render quickly.
fn split_sentences(paragraph: &str) -> Vec<String> {
    let chars: Vec<char> = paragraph.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    for i in 0..chars.len() {
        let ends = matches!(chars[i], '.' | '!' | '?')
            && chars.get(i + 1).is_none_or(|n| n.is_whitespace())
            // "e.g." and "3.5" aren't ends: the next word starts with a capital, a digit or a quote.
            && chars.get(i + 2).is_none_or(|n| n.is_uppercase() || n.is_ascii_digit() || matches!(n, '"' | '\u{201c}' | '(' | '\''));
        if ends {
            push_sentence(&chars[start..=i].iter().collect::<String>(), &mut out);
            start = i + 1;
        }
    }
    if start < chars.len() {
        push_sentence(&chars[start..].iter().collect::<String>(), &mut out);
    }
    out
}

fn push_sentence(sentence: &str, out: &mut Vec<String>) {
    let mut rest = sentence.trim();
    while rest.chars().count() > MAX_SENTENCE {
        let cut: String = rest.chars().take(MAX_SENTENCE).collect();
        let at = cut.rfind(", ").map(|i| i + 1).or_else(|| cut.rfind(' ')).unwrap_or(cut.len());
        out.push(rest[..at].trim().to_string());
        rest = rest[at..].trim();
    }
    if !rest.is_empty() && rest.chars().any(|c| c.is_alphanumeric()) {
        out.push(rest.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_reads_as_plain_sentences() {
        let reply = "## What changed\n\nI fixed the **login** bug in `auth.rs`. See [the issue](https://x.com/1) for details.\n\n- Added a test\n- Kept *snake_case* names\n\n```rust\nfn main() {}\n```\n\n| File | Change |\n|---|---|\n| auth.rs | fixed |\n\nDone at https://example.com/run?id=3, e.g. today.";
        assert_eq!(speakable_sentences(reply), vec![
            "What changed.",
            "I fixed the login bug in auth.rs.",
            "See the issue for details.",
            "Added a test.",
            "Kept snake_case names.",
            "Code block omitted.",
            "File, Change.",
            "auth.rs, fixed.",
            "Done at link e.g. today.",
        ]);
    }

    #[test]
    fn long_sentences_split_at_a_comma() {
        let long = format!("{}, and then it ends.", "word ".repeat(80).trim());
        let sentences = speakable_sentences(&long);
        assert!(sentences.len() >= 2 && sentences.iter().all(|s| s.chars().count() <= MAX_SENTENCE), "{sentences:?}");
    }
}
