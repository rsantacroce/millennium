//! Parses what the player typed into the chat box.
//!
//! ```text
//! hello              -> send on the currently selected channel
//! /s hello           -> Say (/say)
//! /g hello           -> Global (/global)
//! /w Bob hello       -> Whisper (/whisper, /t, /tell)
//! /r thanks          -> reply to the last person who whispered you (/reply)
//! /ignore Bob, /unignore Bob, /ignorelist
//! /mute Bob 10, /unmute Bob   (admins)
//! /help, /clear
//! ```

use super::{ChatAction, ChatTarget};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatInput {
    /// Plain text: send on whatever channel is currently selected in the UI.
    Default(String),
    Send { target: ChatTarget, text: String },
    /// Reply to the last whisper sender (resolved on the client).
    Reply(String),
    Action(ChatAction),
    Help,
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("nothing to send")]
    Empty,
    #[error("unknown command /{0} - type /help")]
    UnknownCommand(String),
    #[error("usage: {0}")]
    Usage(&'static str),
}

pub fn parse_input(input: &str) -> Result<ChatInput, ParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ParseError::Empty);
    }
    let Some(rest) = input.strip_prefix('/') else {
        return Ok(ChatInput::Default(input.to_string()));
    };

    let (cmd, args) = split_word(rest);
    let cmd = cmd.to_ascii_lowercase();
    let text = |usage| non_empty(args).ok_or(ParseError::Usage(usage));

    match cmd.as_str() {
        "s" | "say" => Ok(ChatInput::Send { target: ChatTarget::Say, text: text("/s <message>")? }),
        "g" | "global" => {
            Ok(ChatInput::Send { target: ChatTarget::Global, text: text("/g <message>")? })
        }
        "w" | "whisper" | "t" | "tell" => {
            const USAGE: &str = "/w <name> <message>";
            let (name, msg) = split_word(args);
            let name = non_empty(name).ok_or(ParseError::Usage(USAGE))?;
            let msg = non_empty(msg).ok_or(ParseError::Usage(USAGE))?;
            Ok(ChatInput::Send { target: ChatTarget::Whisper { to: name }, text: msg })
        }
        "r" | "reply" => Ok(ChatInput::Reply(text("/r <message>")?)),
        "ignore" => Ok(ChatInput::Action(ChatAction::Ignore { name: one_name(args, "/ignore <name>")? })),
        "unignore" => {
            Ok(ChatInput::Action(ChatAction::Unignore { name: one_name(args, "/unignore <name>")? }))
        }
        "ignorelist" | "ignored" => Ok(ChatInput::Action(ChatAction::ListIgnored)),
        "mute" => {
            const USAGE: &str = "/mute <name> [minutes]";
            let (name, minutes) = split_word(args);
            let name = non_empty(name).ok_or(ParseError::Usage(USAGE))?;
            let minutes = match minutes.trim() {
                "" => 10,
                m => m.parse().map_err(|_| ParseError::Usage(USAGE))?,
            };
            Ok(ChatInput::Action(ChatAction::Mute { name, minutes }))
        }
        "unmute" => Ok(ChatInput::Action(ChatAction::Unmute { name: one_name(args, "/unmute <name>")? })),
        "help" | "?" => Ok(ChatInput::Help),
        "clear" => Ok(ChatInput::Clear),
        _ => Err(ParseError::UnknownCommand(cmd)),
    }
}

pub const HELP_TEXT: &[&str] = &[
    "/s <msg> - say (nearby)   /g <msg> - global",
    "/w <name> <msg> - whisper   /r <msg> - reply",
    "/ignore <name>, /unignore <name>, /ignorelist",
    "/clear - clear this window   Tab - switch channel",
];

fn split_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim_start()),
        None => (s, ""),
    }
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn one_name(args: &str, usage: &'static str) -> Result<String, ParseError> {
    let (name, extra) = split_word(args);
    if name.is_empty() || !extra.is_empty() {
        return Err(ParseError::Usage(usage));
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(target: ChatTarget, text: &str) -> Result<ChatInput, ParseError> {
        Ok(ChatInput::Send { target, text: text.into() })
    }

    #[test]
    fn plain_text_uses_default_channel() {
        assert_eq!(parse_input("  hey all "), Ok(ChatInput::Default("hey all".into())));
    }

    #[test]
    fn channel_prefixes() {
        assert_eq!(parse_input("/s hi there"), send(ChatTarget::Say, "hi there"));
        assert_eq!(parse_input("/SAY hi"), send(ChatTarget::Say, "hi"));
        assert_eq!(parse_input("/g lfg arcade"), send(ChatTarget::Global, "lfg arcade"));
    }

    #[test]
    fn whisper() {
        let to_bob = || ChatTarget::Whisper { to: "Bob".into() };
        assert_eq!(parse_input("/w Bob meet at the cafe"), send(to_bob(), "meet at the cafe"));
        assert_eq!(parse_input("/tell   Bob   yo"), send(to_bob(), "yo"));
        assert!(matches!(parse_input("/w Bob"), Err(ParseError::Usage(_))));
        assert!(matches!(parse_input("/w"), Err(ParseError::Usage(_))));
    }

    #[test]
    fn social_commands() {
        assert_eq!(
            parse_input("/ignore Spammer"),
            Ok(ChatInput::Action(ChatAction::Ignore { name: "Spammer".into() }))
        );
        assert!(matches!(parse_input("/ignore a b"), Err(ParseError::Usage(_))));
        assert_eq!(
            parse_input("/mute Spammer"),
            Ok(ChatInput::Action(ChatAction::Mute { name: "Spammer".into(), minutes: 10 }))
        );
        assert_eq!(
            parse_input("/mute Spammer 60"),
            Ok(ChatInput::Action(ChatAction::Mute { name: "Spammer".into(), minutes: 60 }))
        );
        assert!(matches!(parse_input("/mute Spammer soon"), Err(ParseError::Usage(_))));
    }

    #[test]
    fn errors() {
        assert_eq!(parse_input("   "), Err(ParseError::Empty));
        assert_eq!(parse_input("/dance"), Err(ParseError::UnknownCommand("dance".into())));
        assert!(matches!(parse_input("/g   "), Err(ParseError::Usage(_))));
    }
}
