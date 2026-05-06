//! Renet channel layout.
//!
//! Renet supports any number of named channels per connection. We use
//! three: one unreliable for snapshots (loss-tolerant; the next
//! snapshot supersedes the lost one), one reliable-ordered for events
//! that must arrive (damage, deaths, loot pickups), and one
//! reliable-ordered for control messages (handshake, floor
//! transitions, errors).

use renet::{ChannelConfig, SendType};

/// Stable channel ids used by both ends of a renet connection. The
/// numeric values are written into renet's [`ChannelConfig`] and must
/// not change without a protocol version bump.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// World snapshots. Unreliable; latest wins.
    Snapshot = 0,
    /// Damage/cast/death/loot events. Reliable ordered.
    Event = 1,
    /// Handshake, lobby, floor transitions, kicks. Reliable ordered.
    Control = 2,
}

impl From<Channel> for u8 {
    fn from(c: Channel) -> u8 {
        c as u8
    }
}

/// Build the channel-config list shared by both client and server
/// renet endpoints. Order doesn't matter; the channel id is what
/// renet uses to route packets.
pub fn channel_config() -> Vec<ChannelConfig> {
    vec![
        ChannelConfig {
            channel_id: Channel::Snapshot as u8,
            // Snapshots ride an unreliable buffer; if we miss one,
            // the next supersedes it. Sized to comfortably hold
            // several seconds of worst-case ticks (a populated
            // rift floor with hundreds of replicated entities)
            // without back-pressure dropping fresh sends — old
            // queued snapshots are still naturally superseded by
            // newer ones.
            max_memory_usage_bytes: 4 * 1024 * 1024,
            send_type: SendType::Unreliable,
        },
        ChannelConfig {
            channel_id: Channel::Event as u8,
            // Events are usually small (damage numbers, hit confirms)
            // but we may burst on AoE casts that hit many enemies.
            max_memory_usage_bytes: 256 * 1024,
            send_type: SendType::ReliableOrdered {
                resend_time: std::time::Duration::from_millis(120),
            },
        },
        ChannelConfig {
            channel_id: Channel::Control as u8,
            // Control is rare but mustn't be dropped. Generous buffer
            // covers a worst-case floor-transition payload.
            max_memory_usage_bytes: 512 * 1024,
            send_type: SendType::ReliableOrdered {
                resend_time: std::time::Duration::from_millis(150),
            },
        },
    ]
}
