//! The RT packet: the mixer's whole state, in one datagram.
//!
//! The TEXT channel is one-way, so this is the only way a client reads
//! anything back. A client subscribes and is sent one of these repeatedly;
//! it diffs them to notice changes. That is why every field is here at a
//! fixed offset rather than being asked for by name.
//!
//! Known gaps, and they are the protocol's rather than ours: neither
//! packet structure carries bus EQ, and there is nowhere in it for the
//! effect knobs. A client that wants those has to send a request and
//! believe it worked. Nothing can be done about that from this end.

/// Where each field sits, measured from the start of the whole packet -
/// header included, because that is how the clients index it.
mod offset {
    pub const KIND: usize = 28;
    pub const BUFFER_SIZE: usize = 30;
    pub const VERSION: usize = 32;
    pub const OPTIONS: usize = 36;
    pub const SAMPLE_RATE: usize = 40;
    pub const STRIP_LEVELS: usize = 44;
    pub const BUS_LEVELS: usize = 112;
    pub const TRANSPORT: usize = 240;
    pub const STRIP_STATE: usize = 244;
    pub const BUS_STATE: usize = 276;
    pub const STRIP_GAIN_LAYERS: usize = 308;
    pub const BUS_GAIN: usize = 436;
    pub const STRIP_LABELS: usize = 452;
    pub const BUS_LABELS: usize = 932;
    pub const END: usize = 1412;
}

/// The whole packet, header included.
pub const PACKET_SIZE: usize = offset::END;

/// How many strip levels the packet has room for. More than we have
/// strips: the field is sized for a mixer whose strips can be
/// multichannel, and the tail is left at silence.
pub const STRIP_LEVEL_SLOTS: usize = 34;

/// How many bus levels it has room for.
pub const BUS_LEVEL_SLOTS: usize = 64;

/// Strips and buses, of which there are eight each.
pub const CHANNELS: usize = 8;

/// How many fader layers a strip has.
pub const LAYERS: usize = 8;

/// How long a label may be.
pub const LABEL_SIZE: usize = 60;

/// What a mixer says it is. Potato is the eight-by-eight one, which is
/// the shape we are.
pub const KIND_POTATO: u8 = 3;

/// The bits of a channel's state word.
///
/// One word per strip and per bus, and a client reads every switch out of
/// it. The values are the protocol's; the names are ours.
pub mod state {
    pub const MUTE: u32 = 0x0000_0001;
    pub const SOLO: u32 = 0x0000_0002;
    pub const MONO: u32 = 0x0000_0004;
    pub const MC: u32 = 0x0000_0008;

    /// A bus's mode lives in these four bits as a number, not as flags.
    pub const MODE_MASK: u32 = 0x0000_00F0;
    /// How far to shift [`MODE_MASK`] to read it as a number.
    pub const MODE_SHIFT: u32 = 4;

    pub const EQ_ON: u32 = 0x0000_0100;
    pub const EQ_AB: u32 = 0x0000_0800;

    /// The eight routing buttons. A5 is out of line with the rest, which
    /// is the protocol's doing and not a transcription slip.
    pub const BUS_A: [u32; 5] = [
        0x0000_1000,
        0x0000_2000,
        0x0000_4000,
        0x0000_8000,
        0x0008_0000,
    ];
    pub const BUS_B: [u32; 3] = [0x0001_0000, 0x0002_0000, 0x0004_0000];

    pub const POST_REVERB: u32 = 0x0100_0000;
    pub const POST_DELAY: u32 = 0x0200_0000;
    pub const POST_FX1: u32 = 0x0400_0000;
    pub const POST_FX2: u32 = 0x0800_0000;

    pub const SEL: u32 = 0x1000_0000;
    pub const MONITOR: u32 = 0x2000_0000;
}

/// Everything a client can read, in the mixer's own terms.
///
/// Filled in by whatever is being controlled and handed to [`payload`].
/// Decibels and switches here; the packing into hundredths and bit fields
/// is this module's business.
#[derive(Debug, Clone)]
pub struct State {
    pub sample_rate: u32,
    pub buffer_size: u16,
    /// Peak level per strip, in dB.
    pub strip_levels: Vec<f32>,
    /// Peak level per bus, in dB.
    pub bus_levels: Vec<f32>,
    /// One state word per strip, built from [`state`].
    pub strip_state: [u32; CHANNELS],
    pub bus_state: [u32; CHANNELS],
    /// Fader positions in dB, per layer then per strip.
    pub strip_gain: [[f32; CHANNELS]; LAYERS],
    pub bus_gain: [f32; CHANNELS],
    pub strip_labels: Vec<String>,
    pub bus_labels: Vec<String>,
    /// Whether the recorder is running, which the protocol keeps as its
    /// own word rather than in a channel's state.
    pub transport: u32,
}

impl Default for State {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            buffer_size: 512,
            strip_levels: Vec::new(),
            bus_levels: Vec::new(),
            strip_state: [0; CHANNELS],
            bus_state: [0; CHANNELS],
            strip_gain: [[0.0; CHANNELS]; LAYERS],
            bus_gain: [0.0; CHANNELS],
            strip_labels: Vec::new(),
            bus_labels: Vec::new(),
            transport: 0,
        }
    }
}

/// Decibels as the packet carries them: hundredths, in a signed short.
///
/// Saturating rather than wrapping. A level below what a short can hold
/// is silence and should read as the quietest thing available, not as a
/// loud one, which is what a wrapping cast would give.
fn db100(value: f32) -> i16 {
    let scaled = value * 100.0;
    if scaled <= f32::from(i16::MIN) {
        i16::MIN
    } else if scaled >= f32::from(i16::MAX) {
        i16::MAX
    } else {
        // Rounded first: truncation would make every negative gain read a
        // hundredth louder than it is.
        scaled.round() as i16
    }
}

/// Write a label into its fixed-width slot.
fn label(out: &mut [u8], text: &str) {
    let bytes = text.as_bytes();
    // Truncated on a character boundary, so a label with an accent in it
    // cannot be cut into something that is not text.
    let mut len = bytes.len().min(LABEL_SIZE - 1);
    while len > 0 && !text.is_char_boundary(len) {
        len -= 1;
    }
    out[..len].copy_from_slice(&bytes[..len]);
}

/// Build the packet's body - everything after the header.
///
/// The offsets above are absolute, so this builds the whole packet and
/// the caller writes its header over the front. That way the offsets read
/// the same here as they do in every client that parses them, which is
/// worth more than avoiding one copy.
#[must_use]
pub fn payload(state: &State) -> Vec<u8> {
    let mut packet = vec![0u8; PACKET_SIZE];

    packet[offset::KIND] = KIND_POTATO;
    packet[offset::BUFFER_SIZE..offset::BUFFER_SIZE + 2]
        .copy_from_slice(&state.buffer_size.to_le_bytes());
    packet[offset::VERSION..offset::VERSION + 4].copy_from_slice(&[0, 1, 0, 0]);
    packet[offset::OPTIONS..offset::OPTIONS + 4].copy_from_slice(&0u32.to_le_bytes());
    packet[offset::SAMPLE_RATE..offset::SAMPLE_RATE + 4]
        .copy_from_slice(&state.sample_rate.to_le_bytes());

    for (i, level) in state.strip_levels.iter().take(STRIP_LEVEL_SLOTS).enumerate() {
        let at = offset::STRIP_LEVELS + i * 2;
        packet[at..at + 2].copy_from_slice(&db100(*level).to_le_bytes());
    }
    for (i, level) in state.bus_levels.iter().take(BUS_LEVEL_SLOTS).enumerate() {
        let at = offset::BUS_LEVELS + i * 2;
        packet[at..at + 2].copy_from_slice(&db100(*level).to_le_bytes());
    }

    packet[offset::TRANSPORT..offset::TRANSPORT + 4]
        .copy_from_slice(&state.transport.to_le_bytes());

    for (i, word) in state.strip_state.iter().enumerate() {
        let at = offset::STRIP_STATE + i * 4;
        packet[at..at + 4].copy_from_slice(&word.to_le_bytes());
    }
    for (i, word) in state.bus_state.iter().enumerate() {
        let at = offset::BUS_STATE + i * 4;
        packet[at..at + 4].copy_from_slice(&word.to_le_bytes());
    }

    for (layer, gains) in state.strip_gain.iter().enumerate() {
        for (i, gain) in gains.iter().enumerate() {
            let at = offset::STRIP_GAIN_LAYERS + layer * CHANNELS * 2 + i * 2;
            packet[at..at + 2].copy_from_slice(&db100(*gain).to_le_bytes());
        }
    }
    for (i, gain) in state.bus_gain.iter().enumerate() {
        let at = offset::BUS_GAIN + i * 2;
        packet[at..at + 2].copy_from_slice(&db100(*gain).to_le_bytes());
    }

    for (i, text) in state.strip_labels.iter().take(CHANNELS).enumerate() {
        let at = offset::STRIP_LABELS + i * LABEL_SIZE;
        label(&mut packet[at..at + LABEL_SIZE], text);
    }
    for (i, text) in state.bus_labels.iter().take(CHANNELS).enumerate() {
        let at = offset::BUS_LABELS + i * LABEL_SIZE;
        label(&mut packet[at..at + LABEL_SIZE], text);
    }

    // The header goes on the front, written by the caller.
    packet
}

#[cfg(test)]
mod tests {
    use super::{CHANNELS, LABEL_SIZE, PACKET_SIZE, State, db100, offset, payload, state};

    #[test]
    fn a_packet_is_the_size_clients_index_into() {
        assert_eq!(payload(&State::default()).len(), PACKET_SIZE);
    }

    #[test]
    fn decibels_are_hundredths() {
        assert_eq!(db100(0.0), 0);
        assert_eq!(db100(-6.0), -600);
        assert_eq!(db100(12.0), 1200);
        // Rounded, not truncated: -6.005 is nearer -6.01 than -6.00.
        assert_eq!(db100(-6.005), -601);
    }

    /// Silence must not wrap round into something loud.
    #[test]
    fn a_level_below_the_floor_saturates() {
        assert_eq!(db100(-100_000.0), i16::MIN);
        assert_eq!(db100(100_000.0), i16::MAX);
        assert_eq!(db100(f32::NEG_INFINITY), i16::MIN);
    }

    #[test]
    fn state_words_land_where_a_client_reads_them() {
        let mut given = State::default();
        given.strip_state[2] = state::MUTE | state::BUS_A[0];
        given.bus_state[1] = state::SEL;
        let packet = payload(&given);

        let at = offset::STRIP_STATE + 2 * 4;
        let read = u32::from_le_bytes(packet[at..at + 4].try_into().expect("four bytes"));
        assert_eq!(read, state::MUTE | state::BUS_A[0]);

        let at = offset::BUS_STATE + 4;
        let read = u32::from_le_bytes(packet[at..at + 4].try_into().expect("four bytes"));
        assert_eq!(read, state::SEL);
    }

    #[test]
    fn gains_are_written_per_layer() {
        let mut given = State::default();
        given.strip_gain[0][0] = -6.0;
        given.strip_gain[7][7] = 3.0;
        given.bus_gain[3] = -12.5;
        let packet = payload(&given);

        let read = |at: usize| i16::from_le_bytes(packet[at..at + 2].try_into().expect("two"));
        assert_eq!(read(offset::STRIP_GAIN_LAYERS), -600);
        assert_eq!(
            read(offset::STRIP_GAIN_LAYERS + 7 * CHANNELS * 2 + 7 * 2),
            300
        );
        assert_eq!(read(offset::BUS_GAIN + 3 * 2), -1250);
    }

    #[test]
    fn labels_are_written_and_terminated() {
        let mut given = State::default();
        given.strip_labels = vec!["Mic IN".to_owned()];
        let packet = payload(&given);
        let at = offset::STRIP_LABELS;
        assert_eq!(&packet[at..at + 6], b"Mic IN");
        assert_eq!(packet[at + 6], 0, "a label needs its terminator");
    }

    /// A long label must not run into the next one's slot.
    #[test]
    fn a_long_label_stays_in_its_slot() {
        let mut given = State::default();
        given.strip_labels = vec!["x".repeat(200), "second".to_owned()];
        let packet = payload(&given);
        assert_eq!(packet[offset::STRIP_LABELS + LABEL_SIZE - 1], 0);
        let at = offset::STRIP_LABELS + LABEL_SIZE;
        assert_eq!(&packet[at..at + 6], b"second");
    }

    /// Cutting UTF-8 mid-character would hand a client bytes that are not
    /// text, and some clients decode strictly.
    #[test]
    fn a_label_is_cut_on_a_character_boundary() {
        let mut given = State::default();
        given.strip_labels = vec!["é".repeat(40)];
        let packet = payload(&given);
        let at = offset::STRIP_LABELS;
        let slot = &packet[at..at + LABEL_SIZE];
        let end = slot.iter().position(|b| *b == 0).unwrap_or(LABEL_SIZE);
        assert!(std::str::from_utf8(&slot[..end]).is_ok());
    }
}
