#!/usr/bin/env python3
"""
Real-time voice-to-text dictation using local Whisper.

Transcribes speech live as you talk — words appear on screen in real time.
Press Ctrl+C to stop. The full transcription is copied to clipboard.

Usage:
    python live.py              # Start live dictation
    python live.py -m small     # Use a more accurate model
    python live.py --chunk 1.5  # Process every 1.5 seconds
"""

import argparse
import sys
import threading
import time
import subprocess
import tempfile
import wave

import numpy as np
import sounddevice as sd


SAMPLERATE = 16000


def save_wav(audio, path):
    """Save float32 audio array to a 16-bit WAV file."""
    audio_int16 = np.clip(audio * 32767, -32768, 32767).astype(np.int16)
    with wave.open(path, "w") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(SAMPLERATE)
        wf.writeframes(audio_int16.tobytes())


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
    parser = argparse.ArgumentParser(description="Real-time voice-to-text dictation")
    parser.add_argument("--model", "-m", type=str, default="base",
                        choices=["base", "small", "medium"],
                        help="Whisper model size (default: base)")
    parser.add_argument("--chunk", "-c", type=float, default=2,
                        help="Process audio every N seconds (default: 2)")
    parser.add_argument("--device", type=int, default=None,
                        help="Audio input device index")
    args = parser.parse_args()

    from faster_whisper import WhisperModel

    print(f"Loading '{args.model}' model...", end=" ", flush=True)
    model = WhisperModel(args.model, device="cpu", compute_type="int8")
    print("ready.\n")

    print("🎤 Listening... Speak now. Press Ctrl+C to stop.\n")
    print("─" * 50)

    audio_buffer = []
    buffer_lock = threading.Lock()
    committed_text = []      # Finalized lines
    committed_samples = 0    # How far we've committed
    tmp_path = tempfile.mktemp(suffix=".wav")
    chunk_samples = int(args.chunk * SAMPLERATE)

    def audio_callback(indata, frames, time_info, status):
        if status:
            print(f"\n  (audio: {status})", file=sys.stderr)
        with buffer_lock:
            audio_buffer.append(indata.copy())

    stream = sd.InputStream(
        samplerate=SAMPLERATE,
        channels=1,
        dtype="float32",
        callback=audio_callback,
        blocksize=int(SAMPLERATE * 0.1),  # 100ms blocks for low latency
    )

    try:
        stream.start()

        while True:
            time.sleep(args.chunk)

            with buffer_lock:
                if not audio_buffer:
                    continue
                all_audio = np.concatenate(audio_buffer, axis=0).flatten()

            new_audio = all_audio[committed_samples:]

            # Need at least 0.3s of new audio
            if len(new_audio) < SAMPLERATE * 0.3:
                continue

            # Skip silence
            if np.max(np.abs(new_audio)) < 0.005:
                committed_samples = len(all_audio)
                continue

            save_wav(new_audio, tmp_path)

            segments, _ = model.transcribe(
                tmp_path,
                beam_size=1,
                vad_filter=True,
                vad_parameters=dict(
                    min_silence_duration_ms=300,
                    speech_pad_ms=100,
                ),
            )
            new_text = "".join(s.text for s in segments).strip()

            if new_text:
                committed_text.append(new_text)
                committed_samples = len(all_audio)

                # Print full text so far
                full = " ".join(committed_text)
                sys.stdout.write(f"\r\033[K{full}")
                sys.stdout.flush()

    except KeyboardInterrupt:
        stream.stop()
        stream.close()

        # Final pass on any unprocessed audio
        with buffer_lock:
            if audio_buffer:
                all_audio = np.concatenate(audio_buffer, axis=0).flatten()
                remaining = all_audio[committed_samples:]
                if len(remaining) > SAMPLERATE * 0.3 and np.max(np.abs(remaining)) > 0.005:
                    save_wav(remaining, tmp_path)
                    segments, _ = model.transcribe(tmp_path, beam_size=1, vad_filter=True)
                    tail = "".join(s.text for s in segments).strip()
                    if tail:
                        committed_text.append(tail)

        final_text = " ".join(committed_text)

        print("\n" + "─" * 50)

        if final_text:
            print(f"\n📝 Final transcription:\n")
            print(final_text)

            tool = copy_to_clipboard(final_text)
            if tool:
                print(f"\n✅ Copied to clipboard (via {tool})")
            else:
                print("\n💡 Install 'wl-copy' or 'xclip' to auto-copy to clipboard")
        else:
            print("\n(No speech detected)")


if __name__ == "__main__":
    main()
