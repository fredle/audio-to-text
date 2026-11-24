# Transcribe Audio & Video to Text

Turn recordings, meetings and videos into text and subtitles on your own PC, with no time limit.

Windows can caption live speech, but it can't turn a file into text. This app
does: drop in a lecture, interview, meeting or voice note and get readable
paragraphs with timestamps, then copy them or save them as TXT, SRT or VTT
subtitles. The listening is done on your PC by a Whisper speech model
(through whisper.cpp), so nothing is uploaded and there are no minutes to buy.

## What it does

- Reads MP3, M4A, WAV, FLAC, AAC, WMA, OGG, MP4, MOV, MKV, WebM, AVI and WMV
  through Windows' own codecs.
- Detects the spoken language, or lets you pick from Whisper's list.
- Shows the transcript as paragraphs joined at sentence ends and pauses, with
  the start time of each, as it is heard.
- Search within the transcript, copy all of it, or save it as text (with
  times), SRT or VTT subtitles. Saved files go next to the recording and never
  replace an existing file.
- Shows progress and the time left, and can stop at any point keeping the text
  so far. The window never freezes while it works.
- Queues many files and offers two models: Small (181 MB, faster) and Large
  (547 MB, slower and more accurate).

It does not tell speakers apart, record from a microphone, translate or
summarise.

Shortcuts: Ctrl+O add files, Ctrl+Enter transcribe, Ctrl+F search (Enter for
the next match, Shift+Enter for the previous one), Ctrl+Shift+C copy all,
Ctrl+S save as text.

## Install

- **Microsoft Store**: search for "Transcribe Audio & Video to Text".
- **GitHub Releases**: download the `Setup.exe` from the
  [latest release](https://github.com/fredle/audio-to-text/releases/latest).
  It installs for the current user without admin rights and keeps itself up to
  date (you can turn that off at the bottom of the window).

## Where data lives

Everything is under `%LOCALAPPDATA%\AudioToText`:

- `settings.json`: the chosen language and model, and the update setting.
- `models\`: the downloaded speech models. Delete them from "More options" in
  the app, or remove the folder; they download again when next needed.

Transcripts are not stored by the app. They stay in the window until you save
or copy them.

## Privacy

- No telemetry, no accounts, no ads.
- Your recordings and transcripts never leave your PC.
- The app makes two kinds of network request:
  1. The first time you transcribe with a model, it downloads that model file
     once from Hugging Face
     (`https://huggingface.co/ggerganov/whisper.cpp/resolve/main/`). The size
     is shown before it starts. After that it works offline.
  2. If automatic updates are on, it checks GitHub Releases about once a day
     for a new version.

## Building

Rust 2024 with CMake and LLVM installed (whisper.cpp is compiled from source).

```
cargo build --release
cargo test --release
cargo run --release -- --shots target/shots [--theme dark]
```
