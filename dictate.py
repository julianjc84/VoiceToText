#!/usr/bin/env python3
"""
Voice-to-text dictation tool using local Whisper model.

Usage:
    python dictate.py              # Record until you press Enter, then transcribe
    python dictate.py --duration 5 # Record for 5 seconds
    python dictate.py --model small # Use a different model (base, small, medium)

The transcribed text is printed to stdout and copied to clipboard if xclip/wl-copy is available.
"""

import argparse
import sys
import threading
import tempfile
import subprocess
import wave
import struct

import numpy as np
import sounddevice as sd


def record_until_enter(samplerate=16000):
    """Record audio until the user presses Enter."""
    print("🎤 Recording... Press ENTER to stop.\n")

    audio_chunks = []
    stop_event = threading.Event()

    def callback(indata, frames, time, status):
        if status:
            print(f"  (audio warning: {status})", file=sys.stderr)
        audio_chunks.append(indata.copy())

    stream = sd.InputStream(
        samplerate=samplerate,
        channels=1,
        dtype="float32",
        callback=callback,
    )

    stream.start()
    input()  # blocks until Enter
    stop_event.set()
    stream.stop()
    stream.close()

    if not audio_chunks:
        print("No audio recorded.", file=sys.stderr)
        sys.exit(1)

    return np.concatenate(audio_chunks, axis=0).flatten()


def record_duration(duration, samplerate=16000):
    """Record audio for a fixed duration in seconds."""
    print(f"🎤 Recording for {duration} seconds...")
    audio = sd.rec(
        int(duration * samplerate),
        samplerate=samplerate,
        channels=1,
        dtype="float32",
    )
    sd.wait()
    return audio.flatten()


def save_wav(audio, path, samplerate=16000):
    """Save float32 audio array to a 16-bit WAV file."""
    audio_int16 = np.clip(audio * 32767, -32768, 32767).astype(np.int16)
    with wave.open(path, "w") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(samplerate)
        wf.writeframes(audio_int16.tobytes())


def transcribe(audio_path, model_size="base"):
    """Transcribe audio file using faster-whisper."""
    from faster_whisper import WhisperModel

    print(f"📝 Transcribing with '{model_size}' model (first run downloads the model)...\n")

    model = WhisperModel(model_size, device="cpu", compute_type="int8")
    segments, info = model.transcribe(audio_path, beam_size=5)

    text_parts = []
    for segment in segments:
        text_parts.append(segment.text)

    return "".join(text_parts).strip()


def copy_to_clipboard(text):
    """Try to copy text to system clipboard."""
    for cmd in [["wl-copy"], ["xclip", "-selection", "clipboard"]]:
        try:
            subprocess.run(cmd, input=text.encode(), check=True, capture_output=True)
            return cmd[0]
        except (FileNotFoundError, subprocess.CalledProcessError):
            continue
    return None


def main():
    parser = argparse.ArgumentParser(description="Voice-to-text dictation using local Whisper")
    parser.add_argument("--duration", "-d", type=float, default=None,
                        help="Record for N seconds (default: record until Enter)")
    parser.add_argument("--model", "-m", type=str, default="base",
                        choices=["base", "small", "medium"],
                        help="Whisper model size (default: base)")
    parser.add_argument("--list-devices", action="store_true",
                        help="List available audio input devices and exit")
    parser.add_argument("--device", type=int, default=None,
                        help="Audio input device index (see --list-devices)")
    args = parser.parse_args()

    if args.list_devices:
        print(sd.query_devices())
        return

    if args.device is not None:
        sd.default.device[0] = args.device

    # Record
    samplerate = 16000
    if args.duration:
        audio = record_duration(args.duration, samplerate)
    else:
        audio = record_until_enter(samplerate)

    # Check if audio has any content
    if np.max(np.abs(audio)) < 0.001:
        print("⚠️  Recording seems silent. Check your microphone.", file=sys.stderr)
        sys.exit(1)

    # Save to temp file and transcribe
    with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as f:
        wav_path = f.name
        save_wav(audio, wav_path, samplerate)

    text = transcribe(wav_path, args.model)

    if not text:
        print("(No speech detected)", file=sys.stderr)
        sys.exit(1)

    # Output
    print("=" * 50)
    print(text)
    print("=" * 50)

    # Copy to clipboard
    tool = copy_to_clipboard(text)
    if tool:
        print(f"\n✅ Copied to clipboard (via {tool})")
    else:
        print("\n💡 Install 'wl-copy' or 'xclip' to auto-copy to clipboard")


if __name__ == "__main__":
    main()
