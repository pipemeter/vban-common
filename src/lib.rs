//! The VBAN wire format, as much of it as a control server needs.
//!
//! VBAN is VB-Audio's audio-over-UDP protocol, and the same packet carries
//! four sub-protocols in its top three format bits. Two of them matter here:
//! TEXT, which carries a line of parameter assignments, and SERVICE, which
//! carries pings and the registration for state packets. Audio and serial
//! are recognised and declined rather than parsed - this crate is how the
//! mixer is *controlled*, not how audio reaches it.
//!
//! Every layout here was read off the protocol as the open clients
//! implement it, and the tests below encode packets and read them back to
//! keep this honest. Nothing is copied from those clients: the header is
//! twenty-eight bytes of documented format, and a struct definition is the
//! only shape it can have.
//!
//! The point of speaking a protocol other people already have clients for
//! is that they become the test harness. A mixer that answers `vban-cmd`
//! or `vban_sendtext` can be driven without touching it.

#![forbid(unsafe_code)]

pub mod parameter;
pub mod rt;

pub use parameter::{Parameter, Target, parse_request};

/// Every VBAN packet starts with this, and it is a fixed size.
///
/// `VBAN` | four format bytes | sixteen-byte stream name | frame counter.
pub const HEADER_SIZE: usize = 4 + 4 + 16 + 4;

/// The magic the first four bytes must hold.
pub const MAGIC: &[u8; 4] = b"VBAN";

/// A stream name is a fixed sixteen bytes, padded with nulls.
pub const STREAM_NAME_SIZE: usize = 16;

/// VB-Audio's registered port, and what every client tries first.
pub const DEFAULT_PORT: u16 = 6980;

/// The largest packet the protocol allows.
pub const MAX_PACKET_SIZE: usize = 1436;

/// Which sub-protocol a packet carries, held in the top three bits of the
/// first format byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubProtocol {
    Audio,
    Serial,
    Text,
    Service,
}

impl SubProtocol {
    /// The mask that isolates the sub-protocol bits.
    const MASK: u8 = 0xE0;

    #[must_use]
    pub fn from_format(format_sr: u8) -> Self {
        match format_sr & Self::MASK {
            0x20 => Self::Serial,
            0x40 => Self::Text,
            0x60 => Self::Service,
            // 0x00 is audio, and the four undefined values are not ours to
            // guess at - treating them as audio means they are declined,
            // which is the right answer for anything we do not know.
            _ => Self::Audio,
        }
    }

    #[must_use]
    pub fn bits(self) -> u8 {
        match self {
            Self::Audio => 0x00,
            Self::Serial => 0x20,
            Self::Text => 0x40,
            Self::Service => 0x60,
        }
    }
}

/// What a SERVICE packet is asking for, held in its third format byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// A ping, which expects a pong carrying who we are.
    Ping,
    /// Subscribe to state packets. Its fourth format byte is how many
    /// seconds the subscription lasts.
    RegisterRt,
    /// A state packet, which is what we send rather than receive.
    RtPacket,
    /// Something else, kept as its number so a log can name it.
    Other(u8),
}

impl Service {
    #[must_use]
    pub fn from_byte(byte: u8) -> Self {
        match byte {
            0 => Self::Ping,
            32 => Self::RegisterRt,
            33 => Self::RtPacket,
            other => Self::Other(other),
        }
    }

    #[must_use]
    pub fn to_byte(self) -> u8 {
        match self {
            Self::Ping => 0,
            Self::RegisterRt => 32,
            Self::RtPacket => 33,
            Self::Other(byte) => byte,
        }
    }
}

/// A packet's header, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub protocol: SubProtocol,
    /// The remaining five bits of the first format byte. For TEXT this is
    /// a baud rate index, which a text command channel does not use.
    pub format_sr: u8,
    pub format_nbs: u8,
    pub format_nbc: u8,
    pub format_bit: u8,
    /// The stream name, trimmed of its null padding. Clients send
    /// `Command1` by default; the name is how one mixer is picked out of
    /// several on a network.
    pub stream_name: String,
    pub frame: u32,
}

impl Header {
    /// Read a header off the front of a datagram.
    ///
    /// Returns nothing for anything too short or not VBAN at all, which on
    /// an open UDP port is most of what arrives.
    #[must_use]
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < HEADER_SIZE || &data[..4] != MAGIC {
            return None;
        }
        let format_sr = data[4];
        let name = &data[8..8 + STREAM_NAME_SIZE];
        let end = name
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(name.len());
        Some(Self {
            protocol: SubProtocol::from_format(format_sr),
            // Only the bits that are not the sub-protocol: the protocol is
            // held once, in its own field, so that a header cannot be built
            // claiming one thing in two places and disagreeing.
            format_sr: format_sr & !SubProtocol::MASK,
            format_nbs: data[5],
            format_nbc: data[6],
            format_bit: data[7],
            // Lossy on purpose: a name is for logs and for matching, and a
            // packet with a bad byte in it should not vanish silently.
            stream_name: String::from_utf8_lossy(&name[..end]).into_owned(),
            frame: u32::from_le_bytes([data[24], data[25], data[26], data[27]]),
        })
    }

    /// Write a header out.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut out = [0u8; HEADER_SIZE];
        out[..4].copy_from_slice(MAGIC);
        // The protocol bits win over whatever the rest of the byte holds,
        // so a header built by hand cannot claim to be two things.
        out[4] = (self.format_sr & !SubProtocol::MASK) | self.protocol.bits();
        out[5] = self.format_nbs;
        out[6] = self.format_nbc;
        out[7] = self.format_bit;
        let name = self.stream_name.as_bytes();
        let len = name.len().min(STREAM_NAME_SIZE);
        out[8..8 + len].copy_from_slice(&name[..len]);
        out[24..28].copy_from_slice(&self.frame.to_le_bytes());
        out
    }

    /// What a SERVICE packet is asking for. Meaningless for the others, so
    /// they get nothing rather than a wrong answer.
    #[must_use]
    pub fn service(&self) -> Option<Service> {
        (self.protocol == SubProtocol::Service).then(|| Service::from_byte(self.format_nbc))
    }
}

/// A whole packet, decoded as far as its sub-protocol allows.
#[derive(Debug, Clone, PartialEq)]
pub enum Packet {
    /// A line of parameter assignments.
    Text { header: Header, body: String },
    /// A service request. The payload is kept for the ones that carry one.
    Service {
        header: Header,
        service: Service,
        /// How long an RT subscription should last, in seconds.
        timeout: u8,
    },
    /// Audio or serial, recognised and not ours.
    Other { header: Header },
}

/// Decode a datagram.
#[must_use]
pub fn parse(data: &[u8]) -> Option<Packet> {
    let header = Header::parse(data)?;
    let body = &data[HEADER_SIZE..];
    Some(match header.protocol {
        SubProtocol::Text => {
            // Trailing nulls are padding, not text. A client that pads to a
            // fixed size would otherwise hand us a parameter name with a
            // null on the end that matches nothing.
            let end = body
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(body.len());
            Packet::Text {
                body: String::from_utf8_lossy(&body[..end]).into_owned(),
                header,
            }
        }
        SubProtocol::Service => {
            let service = Service::from_byte(header.format_nbc);
            Packet::Service {
                timeout: header.format_bit,
                service,
                header,
            }
        }
        _ => Packet::Other { header },
    })
}

/// Build a packet to send back.
#[must_use]
pub fn encode(header: &Header, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_SIZE + body.len());
    out.extend_from_slice(&header.to_bytes());
    out.extend_from_slice(body);
    out
}

/// The header of a pong, which answers a ping.
///
/// A pong is a service packet whose sub-type is the same as a ping's - the
/// two are told apart by which direction it went and by carrying a payload.
#[must_use]
pub fn pong_header(frame: u32) -> Header {
    Header {
        protocol: SubProtocol::Service,
        format_sr: 0,
        format_nbs: 0,
        format_nbc: Service::Ping.to_byte(),
        format_bit: 0,
        stream_name: "PING0".to_owned(),
        frame,
    }
}

/// The header of a reply to a query.
///
/// A client that ends a request with `?` waits for one of these. It is a
/// service packet like the others, told apart by carrying the reply
/// sub-type in the second format byte as well as the third.
#[must_use]
pub fn reply_header(frame: u32) -> Header {
    Header {
        protocol: SubProtocol::Service,
        format_sr: 0,
        // `FNCT_REPLY`, which is what marks this an answer rather than a
        // request of the same kind.
        format_nbs: 0x80,
        format_nbc: 0x02,
        format_bit: 0,
        stream_name: "Request Reply".to_owned(),
        frame,
    }
}

/// The header of a state packet.
#[must_use]
pub fn rt_header(frame: u32) -> Header {
    Header {
        protocol: SubProtocol::Service,
        format_sr: 0,
        format_nbs: 0,
        format_nbc: Service::RtPacket.to_byte(),
        format_bit: 0,
        stream_name: "Voicemeeter-RTP".to_owned(),
        frame,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HEADER_SIZE, Header, MAGIC, Packet, Service, SubProtocol, encode, parse, pong_header,
    };

    fn text_header() -> Header {
        Header {
            protocol: SubProtocol::Text,
            format_sr: 0,
            format_nbs: 0,
            format_nbc: 0,
            // UTF8, which is what the clients send.
            format_bit: 0x10,
            stream_name: "Command1".to_owned(),
            frame: 7,
        }
    }

    #[test]
    fn a_header_reads_back_as_it_was_written() {
        let header = text_header();
        let bytes = header.to_bytes();
        assert_eq!(&bytes[..4], MAGIC);
        assert_eq!(bytes.len(), HEADER_SIZE);
        assert_eq!(Header::parse(&bytes), Some(header));
    }

    /// The sub-protocol lives in bits the stream name does not touch, and
    /// getting the mask wrong would make every text packet look like audio.
    #[test]
    fn the_protocol_bits_survive_a_round_trip() {
        for protocol in [
            SubProtocol::Audio,
            SubProtocol::Serial,
            SubProtocol::Text,
            SubProtocol::Service,
        ] {
            let mut header = text_header();
            header.protocol = protocol;
            let read = Header::parse(&header.to_bytes()).expect("parses");
            assert_eq!(read.protocol, protocol);
        }
    }

    #[test]
    fn a_text_packet_carries_its_line() {
        let bytes = encode(&text_header(), b"Strip[0].Gain=-6.0;");
        match parse(&bytes) {
            Some(Packet::Text { body, header }) => {
                assert_eq!(body, "Strip[0].Gain=-6.0;");
                assert_eq!(header.stream_name, "Command1");
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    /// A client that pads its payload would otherwise hand us a trailing
    /// null inside the last parameter's value.
    #[test]
    fn padding_is_not_part_of_the_text() {
        let mut body = b"Strip[0].Mute=1;".to_vec();
        body.resize(64, 0);
        match parse(&encode(&text_header(), &body)) {
            Some(Packet::Text { body, .. }) => assert_eq!(body, "Strip[0].Mute=1;"),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn a_subscription_carries_its_timeout() {
        let header = Header {
            protocol: SubProtocol::Service,
            format_sr: 0,
            format_nbs: 0,
            format_nbc: Service::RegisterRt.to_byte(),
            format_bit: 15,
            stream_name: "Register-RTP".to_owned(),
            frame: 0,
        };
        match parse(&encode(&header, &[])) {
            Some(Packet::Service {
                service, timeout, ..
            }) => {
                assert_eq!(service, Service::RegisterRt);
                assert_eq!(timeout, 15);
            }
            other => panic!("expected a service packet, got {other:?}"),
        }
    }

    /// An open UDP port receives all sorts of things, and none of them
    /// should be able to take the mixer down.
    #[test]
    fn rubbish_is_declined_rather_than_parsed() {
        assert!(parse(&[]).is_none());
        assert!(parse(b"VBA").is_none());
        assert!(parse(&[0u8; HEADER_SIZE]).is_none(), "no magic");
        // Long enough and correctly marked, but empty: still a valid header.
        assert!(parse(&encode(&text_header(), &[])).is_some());
    }

    #[test]
    fn a_pong_says_who_it_is() {
        let header = pong_header(3);
        assert_eq!(header.stream_name, "PING0");
        assert_eq!(header.service(), Some(Service::Ping));
    }

    /// A client checks the whole packet's length before it will believe a
    /// pong, so this size is a handshake requirement rather than a detail.
    /// A client tells a reply from a request by the second format byte,
    /// not only the third, so both have to be set.
    #[test]
    fn a_reply_is_marked_as_one() {
        let header = super::reply_header(1);
        assert_eq!(header.protocol, SubProtocol::Service);
        assert_eq!(header.format_nbs, 0x80);
        assert_eq!(header.format_nbc, 0x02);
        assert_eq!(header.stream_name, "Request Reply");
        assert_eq!(Header::parse(&header.to_bytes()), Some(header));
    }

    #[test]
    fn a_pong_is_the_size_the_protocol_says() {
        let body = super::pong::payload("pipemeter", "host");
        assert_eq!(body.len(), super::pong::PAYLOAD_SIZE);
        assert_eq!(
            encode(&pong_header(0), &body).len(),
            super::pong::PACKET_SIZE
        );
    }

    /// The first field is how a client decides what it is talking to.
    #[test]
    fn a_pong_calls_itself_a_mixer() {
        let body = super::pong::payload("pipemeter", "host");
        let kind = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
        assert_eq!(kind, super::pong::TYPE_VIRTUAL_MIXER);
    }

    /// A name longer than its field must shorten rather than push every
    /// field after it out of place.
    #[test]
    fn a_long_name_is_truncated_not_overflowed() {
        let body = super::pong::payload(&"x".repeat(500), &"y".repeat(500));
        assert_eq!(body.len(), super::pong::PAYLOAD_SIZE);
    }
}

/// The payload a pong carries: who and what answered.
///
/// A client's login is a ping, and it does not accept a bare header as an
/// answer - it wants the whole description, and it reads our kind out of
/// the first field to decide what it is talking to. An empty pong is not
/// a small pong; it is silence with extra steps, and a real client times
/// out on it.
pub mod pong {
    /// Sizes are the protocol's, not ours, and the total is what a client
    /// checks before it will look at the rest.
    pub const PAYLOAD_SIZE: usize = 676;

    /// The whole packet, header included.
    pub const PACKET_SIZE: usize = super::HEADER_SIZE + PAYLOAD_SIZE;

    /// What kind of thing is answering. A mixer is a virtual mixer, which
    /// is the value a client reads to decide it is talking to something
    /// Voicemeeter-shaped rather than to a Matrix or a bare receptor.
    pub const TYPE_VIRTUAL_MIXER: u32 = 0x0000_0020;

    /// What it can do. Text requests, which is exactly what we serve.
    pub const FEATURE_TEXT: u32 = 0x0001_0000;

    /// Write a fixed-width, null-padded field.
    ///
    /// Truncated rather than refused: these are descriptions, and a long
    /// hostname should shorten rather than fail a handshake.
    fn field(out: &mut Vec<u8>, text: &str, width: usize) {
        let bytes = text.as_bytes();
        let len = bytes.len().min(width.saturating_sub(1));
        out.extend_from_slice(&bytes[..len]);
        out.resize(out.len() + width - len, 0);
    }

    /// Build the payload.
    #[must_use]
    pub fn payload(application: &str, host: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(PAYLOAD_SIZE);
        for value in [
            TYPE_VIRTUAL_MIXER,
            FEATURE_TEXT,
            0,           // no extended features
            48_000,      // preferred rate
            8_000,       // minimum
            192_000,     // maximum
            0x0071_c399, // the colour the mixer is drawn in
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        // Version, as four bytes rather than a number.
        out.extend_from_slice(&[0, 1, 0, 0]);
        out.resize(out.len() + 8 + 8, 0); // GPS and user position
        field(&mut out, "EN", 8);
        out.resize(out.len() + 8 + 64, 0); // reserved, then reserved extended
        out.resize(out.len() + 32, 0); // distant IP, filled in by the asker
        out.extend_from_slice(&0u16.to_le_bytes()); // distant port
        out.extend_from_slice(&0u16.to_le_bytes()); // distant reserved
        field(&mut out, "PipeMeeter", 64); // device
        field(&mut out, "PipeMeeter", 64); // manufacturer
        field(&mut out, application, 64);
        field(&mut out, host, 64);
        field(&mut out, "", 128); // user name
        field(&mut out, "A Voicemeeter-shaped mixer for PipeWire", 128);

        debug_assert_eq!(out.len(), PAYLOAD_SIZE);
        out
    }
}
