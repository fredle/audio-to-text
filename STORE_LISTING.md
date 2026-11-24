# Store listing: Transcribe Audio & Video to Text

## Product name

Transcribe Audio & Video to Text

## Short description

Turn recordings, meetings and videos into text and subtitles on your own PC, with no time limit.

## Description

You have a lecture, an interview, a meeting or a voice note, and you want it as text. Windows can caption speech as it happens, but it can't turn a file into a transcript. Most transcription apps send your recording to a server and count the minutes, then ask you to subscribe when they run out.

This app does the work on your own PC. Drop in an audio or video file and it listens with a Whisper speech model running locally, so nothing is uploaded and there is no limit on length. A three-hour recording is fine.

The transcript comes back as readable paragraphs, joined at the ends of sentences and at pauses, each with the time it starts. It appears as it is heard, with a progress bar and the time left, and you can stop at any point and keep what has been written so far.

Search for a word and jump between matches. Copy the whole transcript, or save it as plain text with times, or as SRT or VTT subtitles for a video player or editor. Saved files go next to the recording and never replace a file that is already there.

The spoken language is detected for you, or you can choose it from the list of nearly 100 languages Whisper knows. Add as many files as you like and they are done one after another. There are two models: Small, a 181 MB download that is quicker, and Large, a 547 MB download that is slower and more accurate. Each downloads once, the first time you use it, with its size shown first; after that the app works offline.

Audio and video are read with the codecs already in Windows: MP3, M4A, WAV, FLAC, AAC, WMA, OGG, MP4, MOV, MKV, WebM, AVI and WMV.

What it doesn't do: it doesn't tell speakers apart, record from a microphone, caption live speech, translate or summarise. How well it transcribes depends on the recording; clear speech gives the best results.

There are no ads, no account and no subscription.

## Product features

- Transcribes audio and video files on your own PC; nothing is uploaded
- No time limit and no minutes to buy: any length, any number of files
- Readable paragraphs joined at sentence ends and pauses, each with its start time
- Text appears as it is heard, with progress and the time left
- Stop at any point and keep the text so far
- Detects the spoken language, or choose from nearly 100 languages
- Search the transcript and step through every match
- Copy the whole transcript in one click
- Save as plain text with times, or as SRT or VTT subtitles
- Saved files never overwrite an existing file
- Queue many files and they are transcribed one after another
- Small model (181 MB) for speed, Large model (547 MB) for accuracy
- Works offline once the model has downloaded
- Reads MP3, M4A, WAV, FLAC, MP4, MOV, MKV, WebM and more with Windows' own codecs
- Light and dark themes that follow Windows
- No account, no ads, no telemetry

## Search terms

- transcribe audio
- audio to text
- video to text
- speech to text
- transcription
- subtitles srt
- whisper offline

## Category and subcategory

Productivity (the Store has no subcategories under Productivity).

## Privacy

Stored locally, under %LOCALAPPDATA%\AudioToText:
- settings.json: the chosen language and model, and whether automatic updates are on.
- models\: the downloaded speech model files.

Transcripts are not stored by the app; they stay in the window until you copy or save them, and saved files go where you put them.

Network calls:
- The speech model is downloaded once from Hugging Face (huggingface.co, the ggerganov/whisper.cpp repository) the first time it is needed. Nothing about you or your files is sent; it is a plain file download.
- If automatic updates are on, the app checks GitHub Releases about once a day for a new version.

No recordings, transcripts, usage data or personal data are ever sent anywhere. There is no telemetry and no account.

## Screenshots

1. Recordings become readable text: a meeting transcript as paragraphs with times.
2. No time limit, no minutes: a 90-minute lecture mid-transcription with the time left.
3. Search it, save subtitles: search matches highlighted and an SRT file saved.
4. Many files, many languages: a queue of files with German detected, using the Large model.
