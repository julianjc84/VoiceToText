#!/usr/bin/env python3
"""Record once, then transcribe with all Whisper models to compare quality and speed."""

import time
import sys
import numpy as np
import sounddevice as sd
from dictate import record_until_enter, save_wav

MODELS = ["base", "small", "medium"]


def find_word_diffs(texts):
    """Find words that differ between transcriptions."""
    word_lists = [t.lower().split() for t in texts]
    if not any(word_lists):
        return []

    # Find words present in some but not all outputs
    all_words = set()
    for wl in word_lists:
        all_words.update(wl)

    differing = set()
    for word in all_words:
        counts = [wl.count(word) for wl in word_lists]
        if len(set(counts)) > 1:
            differing.add(word)
    return differing


def main():
    from faster_whisper import WhisperModel

    # Record once
    samplerate = 16000
    print("Say something and we'll run it through all 3 models.\n")
    audio = record_until_enter(samplerate)

    if np.max(np.abs(audio)) < 0.001:
        print("Recording seems silent. Check your microphone.", file=sys.stderr)
        sys.exit(1)

    wav_path = "/tmp/whisper_compare_test.wav"
    save_wav(audio, wav_path, samplerate)
    duration = len(audio) / samplerate
    print(f"Recorded {duration:.1f}s of audio.\n")
    print("=" * 60)

    results = []

    for model_name in MODELS:
        print(f"\n🔄 Loading '{model_name}' model...")
        t0 = time.time()
        model = WhisperModel(model_name, device="cpu", compute_type="int8")
        load_time = time.time() - t0

        print(f"   Transcribing...")

        t0 = time.time()
        segments, info = model.transcribe(wav_path, beam_size=5)
        text = "".join(s.text for s in segments).strip()
        transcribe_time = time.time() - t0

        results.append({
            "model": model_name,
            "text": text,
            "time": transcribe_time,
            "load_time": load_time,
            "language": info.language,
            "lang_prob": info.language_probability,
        })

        print(f"   Done ({transcribe_time:.1f}s)")
        del model

    # --- Summary ---
    print("\n" + "=" * 60)
    print("  COMPARISON SUMMARY")
    print("=" * 60)

    # Speed table
    fastest = min(r["time"] for r in results)
    print(f"\n  Audio: {duration:.1f}s | Language: {results[0]['language']} "
          f"({results[0]['lang_prob']:.0%} confidence)\n")

    print(f"  {'Model':<10} {'Speed':>8} {'vs fastest':>12}")
    print(f"  {'-'*10} {'-'*8} {'-'*12}")
    for r in results:
        slower = f"{r['time']/fastest:.1f}x" if r["time"] > fastest else "fastest"
        print(f"  {r['model']:<10} {r['time']:>7.1f}s {slower:>12}")

    # Transcriptions
    print(f"\n  {'─' * 56}")
    texts = [r["text"] for r in results]
    all_same = len(set(texts)) == 1

    for r in results:
        print(f"\n  [{r['model']}]")
        print(f"  {r['text']}")

    # Difference analysis
    print(f"\n  {'─' * 56}")
    if all_same:
        print("\n  ✅ All 3 models produced identical output.")
        print("  → The 'base' model is your best bet (fastest, same result).")
    else:
        print("\n  ⚠️  Models produced different results:")
        diffs = find_word_diffs(texts)
        if diffs:
            print(f"  Words that differ: {', '.join(sorted(diffs))}")

        # Show which pairs match
        for i in range(len(results)):
            for j in range(i + 1, len(results)):
                if results[i]["text"] == results[j]["text"]:
                    print(f"  → '{results[i]['model']}' and '{results[j]['model']}' match")

        # Recommend
        if results[1]["text"] == results[2]["text"]:
            print(f"\n  → 'small' and 'medium' agree — 'small' is the sweet spot "
                  f"({results[1]['time']:.1f}s vs {results[2]['time']:.1f}s).")
        else:
            print(f"\n  → 'medium' is likely most accurate but slowest "
                  f"({results[2]['time']:.1f}s).")
            print(f"  → 'small' is a good middle ground ({results[1]['time']:.1f}s).")

    print()


if __name__ == "__main__":
    main()
