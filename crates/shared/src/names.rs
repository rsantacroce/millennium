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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_names() {
        assert_eq!(validate_name("Neo"), Ok(()));
        assert_eq!(validate_name("dial_up_kid99"), Ok(()));
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
