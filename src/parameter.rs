//! The text a request carries: `Strip[0].Gain=-6.0;`.
//!
//! A request is a list of assignments, separated by semicolons or by
//! newlines, and a client may pack as many into one datagram as will fit.
//! The TEXT channel is one-way by design - to read the mixer's state a
//! client subscribes to the state packets instead - so everything here is
//! an assignment, and nothing needs a reply.
//!
//! What a parameter *means* is not decided here. This crate knows that
//! `Strip[0].Gain` addresses field `gain` of strip 0; whether that is
//! decibels, and what happens when it changes, belongs to the mixer. That
//! split keeps the protocol testable without a mixer attached.

/// What a parameter addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// One of the eight input strips, zero-based as the protocol counts.
    Strip(usize),
    /// One of the eight buses.
    Bus(usize),
    /// The transport and the other verbs: `Command.Restart`, `Command.Show`.
    Command,
    /// The tape deck.
    Recorder,
    /// One of the macro buttons.
    Button(usize),
    /// Ours, outside Voicemeeter's tree: `PipeMeeter.Window=reverb`.
    ///
    /// A namespace of our own rather than new fields on `Command`, so a
    /// macro written for the original can never collide with one of these
    /// and nothing here can be mistaken for a parameter the original has.
    App,
    /// Something addressed by a name we do not implement. Kept whole so it
    /// can be logged and answered honestly rather than guessed at.
    Unknown,
}

/// One assignment from a request.
#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    pub target: Target,
    /// The field, lowercased, with its dots kept: `gain`, `mute`, `a1`,
    /// `eq.on`, `mode.amix`. Lowercased because clients differ on case and
    /// the original does not care.
    pub field: String,
    /// The value as it arrived. Text because a label is a string and a gain
    /// is a number, and the mixer knows which it wanted.
    pub value: String,
    /// The whole assignment, for logs.
    pub raw: String,
}

impl Parameter {
    /// The value as a number, for the parameters that are one.
    #[must_use]
    pub fn as_float(&self) -> Option<f32> {
        self.value.trim().parse().ok()
    }

    /// The value as a switch. `1` and `0` are what the protocol uses;
    /// `on`, `off`, `true` and `false` are accepted because people type
    /// requests by hand and the cost of being kind here is nothing.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self.value.trim().to_ascii_lowercase().as_str() {
            "1" | "on" | "true" | "yes" => Some(true),
            "0" | "off" | "false" | "no" => Some(false),
            // A float that is not 0 counts as on: clients send `1.0`.
            other => other.parse::<f32>().ok().map(|value| value != 0.0),
        }
    }
}

/// Split a request into its assignments.
///
/// Anything unparseable is dropped rather than failing the whole request:
/// a datagram with one bad line in it should still apply the good ones,
/// the same way the mixer would rather set seven parameters than none.
#[must_use]
pub fn parse_request(text: &str) -> Vec<Parameter> {
    text.split([';', '\n', '\r'])
        .filter_map(|line| parse_assignment(line.trim()))
        .collect()
}

/// One `name=value`.
fn parse_assignment(line: &str) -> Option<Parameter> {
    if line.is_empty() {
        return None;
    }
    let (name, value) = line.split_once('=')?;
    let (target, field) = parse_name(name.trim())?;
    Some(Parameter {
        target,
        field,
        // Quotes are stripped: a label with a space in it arrives quoted,
        // and the quotes are the protocol's, not part of the name.
        value: value.trim().trim_matches('"').to_owned(),
        raw: line.to_owned(),
    })
}

/// Split `Strip[0].Gain` into what it addresses and which field.
fn parse_name(name: &str) -> Option<(Target, String)> {
    // `Command.Restart` always has a dot; something without one is not a
    // parameter we can place.
    let (head, field) = name.split_once('.')?;
    let field = field.trim().to_ascii_lowercase();
    if field.is_empty() {
        return None;
    }

    let head = head.trim();
    let (kind, index) = match head.split_once('[') {
        Some((kind, rest)) => {
            let digits = rest.strip_suffix(']')?;
            (kind.trim(), Some(digits.trim().parse::<usize>().ok()?))
        }
        None => (head, None),
    };

    let target = match (kind.to_ascii_lowercase().as_str(), index) {
        ("strip", Some(index)) => Target::Strip(index),
        ("bus", Some(index)) => Target::Bus(index),
        ("button", Some(index)) => Target::Button(index),
        // Indexed or not: `Command.Button[3].State` puts the index on the
        // field rather than the head, and the mixer reads it from there.
        ("command", _) => Target::Command,
        ("recorder", None) => Target::Recorder,
        ("pipemeeter", None) => Target::App,
        _ => Target::Unknown,
    };
    Some((target, field))
}

#[cfg(test)]
mod tests {
    use super::{Target, parse_request};

    #[test]
    fn one_assignment() {
        let read = parse_request("Strip[0].Gain=-6.0;");
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].target, Target::Strip(0));
        assert_eq!(read[0].field, "gain");
        assert_eq!(read[0].as_float(), Some(-6.0));
    }

    /// Clients pack a whole scene change into one datagram, and the order
    /// has to survive: two writes to the same parameter mean the last wins.
    #[test]
    fn many_assignments_keep_their_order() {
        let read = parse_request("Bus[1].Mute=1;Bus[1].Mute=0;Strip[2].A1=1");
        assert_eq!(read.len(), 3);
        assert_eq!(read[0].as_bool(), Some(true));
        assert_eq!(read[1].as_bool(), Some(false));
        assert_eq!(read[2].target, Target::Strip(2));
        assert_eq!(read[2].field, "a1");
    }

    #[test]
    fn newlines_separate_as_well_as_semicolons() {
        let read = parse_request("Strip[0].Mute=1\nStrip[1].Mute=1\r\nStrip[2].Mute=1");
        assert_eq!(read.len(), 3);
    }

    #[test]
    fn case_does_not_matter_and_dots_are_kept() {
        let read = parse_request("BUS[0].EQ.On=1;bus[0].mode.amix=1");
        assert_eq!(read[0].target, Target::Bus(0));
        assert_eq!(read[0].field, "eq.on");
        assert_eq!(read[1].field, "mode.amix");
    }

    #[test]
    fn a_label_keeps_its_spaces_and_loses_its_quotes() {
        let read = parse_request("Strip[3].Label=\"Game Audio\";");
        assert_eq!(read[0].value, "Game Audio");
    }

    #[test]
    fn the_verbs_have_no_index() {
        let read = parse_request("Command.Restart=1;Recorder.Record=1");
        assert_eq!(read[0].target, Target::Command);
        assert_eq!(read[0].field, "restart");
        assert_eq!(read[1].target, Target::Recorder);
    }

    /// One bad line should cost that line and nothing else.
    #[test]
    fn rubbish_is_dropped_and_the_rest_applies() {
        let read = parse_request("Strip[0].Gain=-6;garbage;Strip[;=;Strip[1].Gain=-3");
        assert_eq!(read.len(), 2, "{read:?}");
        assert_eq!(read[0].target, Target::Strip(0));
        assert_eq!(read[1].target, Target::Strip(1));
    }

    #[test]
    fn switches_take_the_words_people_type() {
        for (text, expected) in [
            ("1", true),
            ("0", false),
            ("on", true),
            ("OFF", false),
            ("true", true),
            ("1.0", true),
        ] {
            let read = parse_request(&format!("Strip[0].Mute={text}"));
            assert_eq!(read[0].as_bool(), Some(expected), "{text}");
        }
    }

    /// Our own namespace, which the original does not have and so cannot
    /// clash with.
    #[test]
    fn our_own_commands_are_their_own_target() {
        let read = parse_request("PipeMeeter.Window=reverb;pipemeeter.dump=1");
        assert_eq!(read[0].target, Target::App);
        assert_eq!(read[0].field, "window");
        assert_eq!(read[0].value, "reverb");
        assert_eq!(read[1].target, Target::App);
    }

    #[test]
    fn something_we_do_not_implement_is_named_rather_than_guessed() {
        let read = parse_request("Patch.Composite[0]=3;Vban.Instream[0].On=1");
        assert!(read.iter().all(|p| p.target == Target::Unknown));
    }
}
