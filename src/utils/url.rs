pub fn is_url(source: &str) -> bool {
    let source = source.trim();
    source
        .get(..7)
        .is_some_and(|p| p.eq_ignore_ascii_case("http://"))
        || source
            .get(..8)
            .is_some_and(|p| p.eq_ignore_ascii_case("https://"))
}
