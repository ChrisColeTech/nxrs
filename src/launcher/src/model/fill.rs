//! Sentences with blanks: a constant's `{}` filled in order.

/// Fills each `{}` of `template` with the next of `values`.
pub fn fill(template: &str, values: &[&str]) -> String {
    let mut out = String::new();
    let mut rest = template;
    for value in values {
        let Some(at) = rest.find("{}") else { break };
        out.push_str(&rest[..at]);
        out.push_str(value);
        rest = &rest[at + 2..];
    }
    out.push_str(rest);
    out
}

/// "" for one, "s" for any other count.
pub fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}
