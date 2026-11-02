//! Sonara core: everything except the window.
//!
//! The Tauri shell in `src-tauri` is a thin layer over this crate, so all of the audio, library and
//! lyrics logic can be built and tested without a GUI.

pub mod app;
pub mod decode;
pub mod dsp;
pub mod engine;
pub mod error;
pub mod library;
pub mod lyrics;
pub mod meta;
pub mod playlists;
pub mod queue;
pub mod util;

pub use error::{Error, Result};
