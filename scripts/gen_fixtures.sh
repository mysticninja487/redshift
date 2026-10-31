#!/usr/bin/env bash
# Regenerates the small audio fixtures used by sonara-core's tests. Needs ffmpeg with libopus + libmp3lame.
# Fixtures are committed, so contributors don't need ffmpeg just to run `cargo test`.
#
# The main signal is a linear chirp: f(t) = 400 + 1000*t Hz. Because frequency encodes time, a test can
# tell exactly where in the file a decoder is by measuring the dominant frequency of what it returns.
set -euo pipefail
OUT="$(cd "$(dirname "$0")/.." && pwd)/crates/sonara-core/tests/fixtures"
mkdir -p "$OUT"
F="ffmpeg -hide_banner -loglevel error -y"
phase='2*PI*(400*t+500*t*t)'

chirp() { # $1 = sample rate
  echo "aevalsrc='0.5*sin($phase)|0.25*sin($phase)':s=$1:d=3"
}

# 44.1 kHz 16-bit stereo FLAC. Left is twice as loud as right, so channel order can be checked.
$F -f lavfi -i "$(chirp 44100)" -c:a flac -sample_fmt s16 -compression_level 8 "$OUT/chirp_44k_stereo.flac"

# 48 kHz stereo Opus, in both .opus and .ogg containers (the second checks content sniffing).
$F -f lavfi -i "$(chirp 48000)" -c:a libopus -b:a 96k "$OUT/chirp_48k_stereo.opus"
$F -f lavfi -i "$(chirp 48000)" -c:a libopus -b:a 96k -f ogg "$OUT/chirp_48k_stereo_opus.ogg"

# 96 kHz 24-bit mono FLAC, 1 kHz tone: exercises resampling, bit depth and mono handling.
$F -f lavfi -i "aevalsrc='0.5*sin(2*PI*1000*t)':s=96000:d=2" -c:a flac -sample_fmt s32 -bits_per_raw_sample 24 "$OUT/tone_96k_24bit_mono.flac"

# Mono Opus, 440 Hz.
$F -f lavfi -i "aevalsrc='0.5*sin(2*PI*440*t)':s=48000:d=2" -c:a libopus -b:a 48k "$OUT/tone_48k_mono.opus"

# MP3 has encoder delay/padding, which exercises the packet trim logic.
$F -f lavfi -i "$(chirp 44100)" -c:a libmp3lame -b:a 192k "$OUT/chirp_44k_stereo.mp3"

ls -l "$OUT"
