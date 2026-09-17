//! Keep machine-readable stage envelopes out of ordinary chat, including partial streams.
pub fn stage_prose(text: &str) -> &str {
    const TAG: &str = "<foundry-result>";
    if let Some((prose, _)) = text.split_once(TAG) {
        return prose.trim_end();
    }
    for n in (1..TAG.len()).rev() {
        if text.ends_with(&TAG[..n]) {
            return text[..text.len() - n].trim_end();
        }
    }
    text
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hides_envelopes_at_every_stream_boundary() {
        let text = "Readable summary\n<foundry-result>{\"outcome\":\"passed\"}</foundry-result>";
        let start = text.find('<').unwrap();
        for end in start + 1..=text.len() {
            assert_eq!(stage_prose(&text[..end]), "Readable summary");
        }
        assert_eq!(stage_prose("ordinary text"), "ordinary text");
    }
}
