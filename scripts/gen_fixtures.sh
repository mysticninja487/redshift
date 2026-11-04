#!/usr/bin/env bash
# Regenerates the small audio fixtures used by redshift-core's tests. Needs ffmpeg with libopus + libmp3lame.
# Fixtures are committed, so contributors don't need ffmpeg just to run `cargo test`.
#
# The main signal is a linear chirp: f(t) = 400 + 1000*t Hz. Because frequency encodes time, a test can
# tell exactly where in the file a decoder is by measuring the dominant frequency of what it returns.
set -euo pipefail
OUT="$(cd "$(dirname "$0")/.." && pwd)/crates/redshift-core/tests/fixtures"
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

# --- tagged files for the metadata/library tests ---
# A small cover image, embedded in the FLAC and the MP3.
$F -f lavfi -i "color=c=0x3366cc:s=64x64:d=1" -frames:v 1 "$OUT/_cover.png"
LYRICS=$'[00:01.00]First line\n[00:02.50]Second line'
S="aevalsrc='0.3*sin(2*PI*660*t)':s=44100:d=1"

$F -f lavfi -i "$S" -i "$OUT/_cover.png" -map 0:a -map 1:v -c:a flac -c:v copy -disposition:v attached_pic \
  -metadata title="Tagged Song" -metadata artist="The Testers" -metadata album="Fixture Album" \
  -metadata album_artist="Various Testers" -metadata genre="Test" -metadata date="2021-05-06" \
  -metadata track="3/12" -metadata disc="2/2" \
  -metadata REPLAYGAIN_TRACK_GAIN="-6.50 dB" -metadata REPLAYGAIN_TRACK_PEAK="0.9" \
  -metadata REPLAYGAIN_ALBUM_GAIN="-5.25 dB" -metadata LYRICS="$LYRICS" "$OUT/tagged.flac"

$F -f lavfi -i "aevalsrc='0.3*sin(2*PI*660*t)':s=48000:d=1" -c:a libopus -b:a 64k \
  -metadata title="Opus Song" -metadata artist="Opus Artist" -metadata album="Opus Album" \
  -metadata date="1999" -metadata track="7" "$OUT/tagged.opus"

$F -f lavfi -i "$S" -i "$OUT/_cover.png" -map 0:a -map 1:v -c:a libmp3lame -b:a 128k -c:v copy -id3v2_version 3 \
  -metadata title="MP3 Song" -metadata artist="MP3 Artist" -metadata album="MP3 Album" \
  -metadata date="2010" -metadata track="1/10" "$OUT/tagged.mp3"

# Untagged file: everything must fall back gracefully.
$F -f lavfi -i "$S" -c:a flac -map_metadata -1 "$OUT/Untagged Name.flac"
rm -f "$OUT/_cover.png"

ls -l "$OUT"
