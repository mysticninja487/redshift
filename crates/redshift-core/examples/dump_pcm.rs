//! Developer tool: decode a file with redshift-core and write raw interleaved f32le PCM.
//!
//!     cargo run --release --example dump_pcm -- song.opus out.f32 [seek_seconds]
//!
//! Prints the stream info and frame count to stderr. Handy for comparing against ffmpeg.
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: dump_pcm <input> <output.f32> [seek_seconds]");
        std::process::exit(2);
    }
    let mut dec = redshift_core::decode::open(std::path::Path::new(&args[1])).expect("open");
    if let Some(secs) = args.get(3) {
        dec.seek(secs.parse().expect("seek seconds")).expect("seek");
    }
    let info = dec.info().clone();
    let mut pcm = Vec::new();
    while dec.read(&mut pcm).expect("decode") {}
    eprintln!("{info:?}\nframes decoded: {}", pcm.len() / info.channels);
    let mut out = std::io::BufWriter::new(std::fs::File::create(&args[2]).expect("create output"));
    for s in &pcm {
        out.write_all(&s.to_le_bytes()).unwrap();
    }
}
