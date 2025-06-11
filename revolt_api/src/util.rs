/// Utility functions or macros can go here.
pub fn build_url(base: &str, segments: &[&str]) -> String {
    let mut url = base.trim_end_matches('/').to_string();
    for seg in segments {
        url.push('/');
        url.push_str(seg.trim_start_matches('/'));
    }
    url
}
