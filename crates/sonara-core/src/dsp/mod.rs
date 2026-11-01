//! Signal processing that runs on the audio thread: equaliser, spectrum analysis, resampling.

pub mod eq;
pub mod resample;
pub mod spectrum;

pub use eq::{EqParams, Equalizer, BAND_COUNT, BAND_FREQS};
