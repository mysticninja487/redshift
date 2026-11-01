//! Playback: the real-time mixer, decoder-fed sources, the output device, and the player that
//! ties them to a queue.

pub mod mixer;
pub mod output;
pub mod player;
pub mod source;

pub use mixer::{Command, Event, FrameSource, Mixer, MixerPorts, Shared, Track};
pub use player::{resolve_gain, OfflineConfig, Player, PlayerConfig, PlayerEvent, PlayerState, QueueView, ReplayGainMode, Status};
