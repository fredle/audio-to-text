# Transcribe Audio & Video to Text

Turn recordings, meetings and videos into text and subtitles on your own PC, with no time limit.

## Problem

People have a lecture, interview, meeting or voice note and want it as text. Windows only does live speech (voice typing and Live Captions), not files. The two Store leaders are both under two years old and meter minutes behind a subscription; their reviews complain of paywalls, of minutes offered in exchange for a review, and of a recording lost when the paywall appeared. The one honest offline app is a 2.5 GB download and its reviews fault transcripts chopped into 5 to 10 second lines.

## Features (1.0)

- Drop in audio or video files (MP3, M4A, WAV, FLAC, MP4, MOV, MKV, WebM); the sound is decoded through Windows' own codecs, as MP4 to MP3 & Audio Converter already does
- Transcription runs on the PC with a Whisper model through whisper.cpp, using the whisper-rs crate (CPU only). This is the one native dependency the app is allowed; CMake and LLVM are installed on the build machine and on the CI runner. Nothing is uploaded
- Audio is decoded to 16 kHz mono float with Media Foundation through the windows crate; read C:/devel/code/mp3-converter/src/mf.rs for a working source-reader pattern to adapt
- The app download stays small: the speech model is fetched once on first use from https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ (ggml-small-q5_1.bin, about 181 MB), with its size shown before the download starts, a progress bar, resume or clean retry on failure, and a size check before use. It is stored under the app's data folder and the app then works offline. This download and the update check are the only network calls
- Language is detected automatically or picked from a list
- Transcript shown as readable paragraphs with timestamps, built by joining Whisper's short segments at sentence ends and pauses
- Search within the transcript, copy all, and export as TXT, SRT or VTT, never overwriting an existing file
- Progress with time remaining, and cancel at any point keeping the text so far; transcription runs on a worker thread so the window never freezes
- Free: any file of any length, one at a time, with the Small model. A one-off unlock (licence key, checked offline) adds a queue of many files and the larger, more accurate model (ggml-large-v3-turbo-q5_0.bin, about 547 MB)
- Licence module modelled on C:/devel/code/mp3-converter/src/license.rs (read that file): an Ed25519-signed key checked offline, with its own key prefix ATT1. Leave BUY_URL empty in 1.0, so the limits are off, the queue and larger model are available to everyone, and no Buy or licence UI shows until a licence server exists; embed a well-formed placeholder public key marked to be replaced
- No ads, no review prompt, no account

## Non-goals

- No telling speakers apart in 1.0; the listing says so plainly
- No microphone recording and no built-in audio player in 1.0
- No live captions or dictation into other apps, which Windows already does
- No cloud transcription, translation or AI summaries
- No accuracy percentage claims
- No subscription or minute limits
- No GPU acceleration in 1.0

## Evidence of demand

- https://apps.microsoft.com/detail/9pp0jqlbrmf8
- https://apps.microsoft.com/detail/9n7wbb04192f
- https://apps.microsoft.com/detail/9pgq3h6jxl4c
- https://techcommunity.microsoft.com/discussions/windows11/how-can-i-do-audio-transcription-with-ai-on-windows-11/4562849
