//! Playback: the real-time mixer, decoder-fed sources, the output device, and the player that
//! ties them to a queue.

pub mod mixer;

pub use mixer::{Command, Event, FrameSource, Mixer, MixerPorts, Shared, Track};
