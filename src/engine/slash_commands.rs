/// Parse "/cmd args" -> Some(("cmd", "args")) or None
pub fn parse(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    
    // Check if it's just a "/", which is not a valid command
    if trimmed.len() == 1 {
        return None;
    }

    let without_slash = &trimmed[1..];
    match without_slash.split_once(char::is_whitespace) {
        Some((cmd, args)) => Some((cmd, args.trim())),
        None => Some((without_slash, "")),
    }
}
