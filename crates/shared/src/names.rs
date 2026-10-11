//! Username / character-name rules.

pub const MIN_NAME_LEN: usize = 3;
pub const MAX_NAME_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    #[error("name must be {MIN_NAME_LEN}-{MAX_NAME_LEN} characters")]
    Length,
    #[error("name must start with a letter")]
    FirstChar,
    #[error("name may only contain letters, digits and _")]
    Charset,
}

/// Validates a username. Names are compared case-insensitively everywhere (the DB uses `citext`).
pub fn validate_name(name: &str) -> Result<(), NameError> {
    let len = name.chars().count();
    if !(MIN_NAME_LEN..=MAX_NAME_LEN).contains(&len) {
        return Err(NameError::Length);
    }
    if !name.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return Err(NameError::FirstChar);
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(NameError::Charset);
    }
    Ok(())
}

/// Crew names: 3-20 letters, digits, `_` or spaces, starting with a letter.
pub fn validate_crew_name(name: &str) -> Result<(), &'static str> {
    let len = name.chars().count();
    if !(3..=20).contains(&len) {
        return Err("crew names are 3-20 characters");
    }
    if !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return Err("crew names start with a letter");
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ' ') || name.contains("  ") {
        return Err("crew names may use letters, digits, _ and single spaces");
    }
    Ok(())
}

/// Crew tags: 2-4 letters or digits, shown in capitals.
pub fn validate_crew_tag(tag: &str) -> Result<(), &'static str> {
    if !(2..=4).contains(&tag.len()) || !tag.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("crew tags are 2-4 letters or digits");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_names() {
        assert_eq!(validate_name("Neo"), Ok(()));
        assert_eq!(validate_name("dial_up_kid99"), Ok(()));
    }

    #[test]
    fn crew_names_and_tags() {
        assert!(validate_crew_name("Wharf Rats").is_ok());
        assert!(validate_crew_name("WR").is_err());
        assert!(validate_crew_name("9 Lives").is_err());
        assert!(validate_crew_name("Bad  Spacing").is_err());
        assert!(validate_crew_tag("WRC").is_ok());
        assert!(validate_crew_tag("W").is_err());
        assert!(validate_crew_tag("W-RC").is_err());
    }

    #[test]
    fn rejects_bad_names() {
        assert_eq!(validate_name("ab"), Err(NameError::Length));
        assert_eq!(validate_name("a_very_long_name_indeed"), Err(NameError::Length));
        assert_eq!(validate_name("1337"), Err(NameError::FirstChar));
        assert_eq!(validate_name("_neo"), Err(NameError::FirstChar));
        assert_eq!(validate_name("neo smith"), Err(NameError::Charset));
        assert_eq!(validate_name("néo"), Err(NameError::Charset));
    }
}
