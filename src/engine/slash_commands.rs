/// Parse "/cmd args" -> Some(("cmd", "args")) or None
pub fn parse(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim();
    // Fix #10: Use strip_prefix for idiomatic Rust
    let without_slash = trimmed.strip_prefix('/')?;

    // Check if it's just a "/", which is not a valid command
    if without_slash.is_empty() {
        return None;
    }

    match without_slash.split_once(char::is_whitespace) {
        Some((cmd, args)) => Some((cmd, args.trim())),
        None => Some((without_slash, "")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_command() {
        assert_eq!(parse("/help"), Some(("help", "")));
    }

    #[test]
    fn parse_command_with_args() {
        assert_eq!(parse("/model gpt-4o"), Some(("model", "gpt-4o")));
    }

    #[test]
    fn parse_command_with_multi_word_args() {
        assert_eq!(
            parse("/compact custom prompt here"),
            Some(("compact", "custom prompt here"))
        );
    }

    #[test]
    fn parse_not_a_slash_command() {
        assert_eq!(parse("hello world"), None);
    }

    #[test]
    fn parse_bare_slash() {
        assert_eq!(parse("/"), None);
    }

    #[test]
    fn parse_leading_trailing_whitespace() {
        assert_eq!(parse("  /help  "), Some(("help", "")));
    }

    #[test]
    fn parse_args_trimmed() {
        assert_eq!(parse("/model   gpt-4o  "), Some(("model", "gpt-4o")));
    }

    #[test]
    fn parse_empty_string() {
        assert_eq!(parse(""), None);
    }

    #[test]
    fn parse_whitespace_only() {
        assert_eq!(parse("   "), None);
    }

    #[test]
    fn parse_tab_separated_args() {
        assert_eq!(parse("/cmd\targ1"), Some(("cmd", "arg1")));
    }
}
