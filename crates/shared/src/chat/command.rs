//! Parses what the player typed into the chat box.
//!
//! ```text
//! hello              -> send on the currently selected channel
//! /s hello           -> Say (/say)
//! /g hello           -> Global (/global)
//! /z hello           -> everyone in your zone (/zone)
//! /p hello           -> your party (/party)
//! /c hello           -> your crew (/crew)
//! /w Bob hello       -> Whisper (/whisper, /t, /tell)
//! /r thanks          -> reply to the last person who whispered you (/reply)
//! /ignore Bob, /unignore Bob, /ignorelist
//! /mute Bob 10, /unmute Bob   (admins)
//! /give walkman 2, /givecar hatchback, /givecash 500, /givexp hacking 400   (admins, for testing)
//! /help, /clear
//! ```

use super::{ChatAction, ChatTarget};
use crate::protocol::SocialAction;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatInput {
    /// Plain text: send on whatever channel is currently selected in the UI.
    Default(String),
    Send { target: ChatTarget, text: String },
    /// Reply to the last whisper sender (resolved on the client).
    Reply(String),
    Action(ChatAction),
    /// Trading, party, friend and crew commands.
    Social(SocialAction),
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
        "z" | "zone" => Ok(ChatInput::Send { target: ChatTarget::Zone, text: text("/z <message>")? }),
        "p" | "party" => Ok(ChatInput::Send { target: ChatTarget::Party, text: text("/p <message>")? }),
        "c" | "crew" if !matches!(split_word(args).0, "create" | "invite" | "leave" | "kick" | "disband") => {
            Ok(ChatInput::Send { target: ChatTarget::Crew, text: text("/c <message>")? })
        }
        "crew" | "c" => {
            let (sub, rest) = split_word(args);
            let social = match sub {
                "create" => {
                    const USAGE: &str = "/crew create <Name> <TAG>";
                    // The tag is the last word, so names can have spaces ("Wharf Rats WRC").
                    let (name, tag) = rest.trim().rsplit_once(' ').ok_or(ParseError::Usage(USAGE))?;
                    let tag = non_empty(tag).ok_or(ParseError::Usage(USAGE))?;
                    let name = non_empty(name).ok_or(ParseError::Usage(USAGE))?;
                    SocialAction::CrewCreate { name, tag }
                }
                "invite" => SocialAction::CrewInvite { name: one_name(rest, "/crew invite <name>")? },
                "kick" => SocialAction::CrewKick { name: one_name(rest, "/crew kick <name>")? },
                "leave" => SocialAction::CrewLeave,
                _ => SocialAction::CrewDisband,
            };
            Ok(ChatInput::Social(social))
        }
        "trade" => Ok(ChatInput::Social(SocialAction::TradeRequest { name: one_name(args, "/trade <name>")? })),
        "invite" | "inv" => Ok(ChatInput::Social(SocialAction::PartyInvite { name: one_name(args, "/invite <name>")? })),
        "leave" => Ok(ChatInput::Social(SocialAction::PartyLeave)),
        "kick" => Ok(ChatInput::Social(SocialAction::PartyKick { name: one_name(args, "/kick <name>")? })),
        "friend" | "f" => {
            const USAGE: &str = "/friend add|remove <name>";
            let (sub, rest) = split_word(args);
            let name = one_name(rest, USAGE)?;
            match sub {
                "add" => Ok(ChatInput::Social(SocialAction::FriendAdd { name })),
                "remove" | "rm" => Ok(ChatInput::Social(SocialAction::FriendRemove { name })),
                _ => Err(ParseError::Usage(USAGE)),
            }
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
        "give" => {
            const USAGE: &str = "/give <item id> [quantity]";
            let (item, qty) = split_word(args);
            let item = non_empty(item).ok_or(ParseError::Usage(USAGE))?;
            let qty = match qty.trim() {
                "" => 1,
                q => q.parse().map_err(|_| ParseError::Usage(USAGE))?,
            };
            Ok(ChatInput::Action(ChatAction::Give { item, qty }))
        }
        "givexp" => {
            const USAGE: &str = "/givexp <skill> <amount>";
            let (skill, amount) = split_word(args);
            let skill = non_empty(skill).ok_or(ParseError::Usage(USAGE))?;
            let amount = amount.trim().parse().map_err(|_| ParseError::Usage(USAGE))?;
            Ok(ChatInput::Action(ChatAction::GiveXp { skill, amount }))
        }
        "givecash" => {
            let amount = args.trim().parse().map_err(|_| ParseError::Usage("/givecash <amount>"))?;
            Ok(ChatInput::Action(ChatAction::GiveCash { amount }))
        }
        "givecar" => Ok(ChatInput::Action(ChatAction::GiveCar { model: one_name(args, "/givecar <model>")? })),
        "help" | "?" => Ok(ChatInput::Help),
        "clear" => Ok(ChatInput::Clear),
        _ => Err(ParseError::UnknownCommand(cmd)),
    }
}

pub const HELP_TEXT: &[&str] = &[
    "/s <msg> - say (nearby)   /z <msg> - zone   /g <msg> - global",
    "/p <msg> - party   /c <msg> - crew",
    "/trade <name>   /invite <name>, /leave, /kick <name> (party)",
    "/friend add|remove <name>   /crew create <Name> <TAG>, /crew invite|kick <name>, /crew leave|disband",
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
    fn group_channels() {
        assert_eq!(parse_input("/z anyone at the docks?"), send(ChatTarget::Zone, "anyone at the docks?"));
        assert_eq!(parse_input("/p pull the next one"), send(ChatTarget::Party, "pull the next one"));
        assert_eq!(parse_input("/c meeting at 9"), send(ChatTarget::Crew, "meeting at 9"));
    }

    #[test]
    fn social_commands_parse() {
        let social = |a| Ok(ChatInput::Social(a));
        assert_eq!(parse_input("/trade Bob"), social(SocialAction::TradeRequest { name: "Bob".into() }));
        assert_eq!(parse_input("/invite Bob"), social(SocialAction::PartyInvite { name: "Bob".into() }));
        assert_eq!(parse_input("/leave"), social(SocialAction::PartyLeave));
        assert_eq!(parse_input("/friend add Bob"), social(SocialAction::FriendAdd { name: "Bob".into() }));
        assert!(matches!(parse_input("/friend hug Bob"), Err(ParseError::Usage(_))));
        assert_eq!(
            parse_input("/crew create Wharf Rats WRC"),
            social(SocialAction::CrewCreate { name: "Wharf Rats".into(), tag: "WRC".into() })
        );
        assert_eq!(parse_input("/crew invite Bob"), social(SocialAction::CrewInvite { name: "Bob".into() }));
        assert_eq!(parse_input("/crew leave"), social(SocialAction::CrewLeave));
        // `/c` followed by anything else is crew chat.
        assert_eq!(parse_input("/c who's online?"), send(ChatTarget::Crew, "who's online?"));
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
