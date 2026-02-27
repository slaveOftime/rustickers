use std::path::Path;

pub fn is_url(source: &str) -> bool {
    let source = source.trim();
    source
        .get(..7)
        .is_some_and(|p| p.eq_ignore_ascii_case("http://"))
        || source
            .get(..8)
            .is_some_and(|p| p.eq_ignore_ascii_case("https://"))
        || source
            .get(..7)
            .is_some_and(|p| p.eq_ignore_ascii_case("file://"))
        || source
            .get(..8)
            .is_some_and(|p| p.eq_ignore_ascii_case("local://"))
}

pub fn create_local_file_url(path: &Path) -> Result<String, String> {
    let path = path.to_string_lossy().trim().to_string();
    Ok(format!("local://localhost/{}", path))
}
