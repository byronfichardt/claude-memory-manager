use crate::models::memory::Frontmatter;

/// Parse a markdown file's content into optional frontmatter and body.
pub fn parse(content: &str) -> (Option<Frontmatter>, String) {
    let trimmed = content.trim_start();

    if !trimmed.starts_with("---") {
        return (None, content.to_string());
    }

    // Find the closing ---
    let after_first = &trimmed[3..];
    let closing = after_first.find("\n---");

    match closing {
        Some(pos) => {
            let yaml_str = &after_first[..pos].trim();
            let body_start = 3 + pos + 4; // skip past \n---
            let body = if body_start < trimmed.len() {
                trimmed[body_start..].trim_start_matches('\n').to_string()
            } else {
                String::new()
            };

            match serde_yaml::from_str::<Frontmatter>(yaml_str) {
                Ok(fm) => (Some(fm), body),
                Err(_) => (None, content.to_string()),
            }
        }
        None => (None, content.to_string()),
    }
}

/// Serialize frontmatter and body back into a markdown file's content.
#[allow(dead_code)]
pub fn serialize(frontmatter: &Frontmatter, body: &str) -> String {
    let yaml = serde_yaml::to_string(frontmatter).unwrap_or_default();
    // serde_yaml adds a trailing newline, trim it
    let yaml = yaml.trim_end();

    if body.is_empty() {
        format!("---\n{}\n---\n", yaml)
    } else {
        format!("---\n{}\n---\n\n{}\n", yaml, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::memory::MemoryType;

    #[test]
    fn test_parse_with_frontmatter() {
        let content = r#"---
name: Test memory
description: A test memory file
type: feedback
---

This is the body content.

**Why:** Testing.

**How to apply:** In tests."#;

        let (fm, body) = parse(content);
        let fm = fm.expect("should parse frontmatter");
        assert_eq!(fm.name, "Test memory");
        assert_eq!(fm.description, "A test memory file");
        assert_eq!(fm.memory_type, MemoryType::Feedback);
        assert!(body.contains("This is the body content."));
        assert!(body.contains("**Why:**"));
    }

    #[test]
    fn test_parse_without_frontmatter() {
        let content = "# Just a heading\n\nSome content.";
        let (fm, body) = parse(content);
        assert!(fm.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn test_roundtrip() {
        let fm = Frontmatter {
            name: "Test".to_string(),
            description: "A test".to_string(),
            memory_type: MemoryType::User,
            created: None,
            updated: None,
            last_accessed: None,
            access_count: None,
        };
        let body = "Content here.";
        let serialized = serialize(&fm, body);
        let (parsed_fm, parsed_body) = parse(&serialized);
        let parsed_fm = parsed_fm.expect("should roundtrip");
        assert_eq!(parsed_fm.name, "Test");
        assert_eq!(parsed_body.trim(), body);
    }
}
