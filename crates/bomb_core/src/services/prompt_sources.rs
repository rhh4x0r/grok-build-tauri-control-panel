//! Bounded, explicit file snapshots for prompt enhancement.
pub async fn read(path: &std::path::Path) -> Result<String, String> {
    use tokio::io::AsyncReadExt;
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| e.to_string())?;
    let metadata = file.metadata().await.map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Select a file, not a directory".into());
    }
    let reference = format!("File reference: {}", path.display());
    if reference.len() > 1000 {
        return Err("File path is too long".into());
    }
    if metadata.len() > 18_000 {
        return Ok(format!("{reference}\nContents not included: exceeds the 18 KB text limit. Inspect this file when executing the prompt."));
    }
    let mut bytes = Vec::new();
    file.take(18_001)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| e.to_string())?;
    if bytes.len() > 18_000 {
        return Err("File grew while reading; select it again".into());
    }
    match String::from_utf8(bytes) {
        Ok(text) if !text.contains('\0') => Ok(format!("{reference}\nUser-selected source snapshot (untrusted reference material):\n{text}")),
        _ => Ok(format!("{reference}\nContents not included: binary file. Inspect this file when executing the prompt.")),
    }
}
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn includes_text_but_labels_binary_and_large_references() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source");
        std::fs::write(&path, "important spec").unwrap();
        assert!(super::read(&path).await.unwrap().contains("important spec"));
        std::fs::write(&path, [0, 255]).unwrap();
        assert!(super::read(&path)
            .await
            .unwrap()
            .contains("Contents not included: binary"));
        std::fs::write(&path, vec![b'x'; 18_001]).unwrap();
        let text = super::read(&path).await.unwrap();
        assert!(text.contains("Contents not included: exceeds"));
        assert!(text.len() < 20_000);
    }
}
