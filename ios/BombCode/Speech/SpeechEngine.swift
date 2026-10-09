import AVFoundation
import Foundation

/// Reading replies aloud with Apple's on-device voices: free, offline, no network speech.
///
/// `AVSpeechSynthesizer.speak` has no timeline, so this doesn't play through it. Sentences are
/// rendered with `write(_:toBufferCallback:)` into a cached audio file (raw 32-bit float, mono),
/// playback starts once the first sentence is in and rendering stays ahead of the playhead, and an
/// `AVAudioEngine` with a time-pitch unit plays the file, so seeking is exact and speed keeps the
/// voice's pitch. Shared with the Mac's helper (`tools/bomb-speak`); keep it free of app code.

/// A voice to read with.
struct SpeechVoiceInfo: Codable, Hashable {
    let id: String
    let name: String
    let language: String
    /// "premium", "enhanced" or "default".
    let quality: String
    let personal: Bool
}

enum SpeechVoices {
    /// Installed voices: the person's language first, then by quality and name; novelty voices left out.
    static func all() -> [SpeechVoiceInfo] {
        let language = Locale.preferredLanguages.first.map { String($0.prefix(2)) } ?? "en"
        let voices = AVSpeechSynthesisVoice.speechVoices().filter { voice in
            if #available(iOS 17.0, macOS 14.0, *), voice.voiceTraits.contains(.isNoveltyVoice) { return false }
            return true
        }
        let rank = { (q: String) in q == "premium" ? 0 : q == "enhanced" ? 1 : 2 }
        return voices.map(info).sorted { a, b in
            let (la, lb) = (a.language.hasPrefix(language), b.language.hasPrefix(language))
            if la != lb { return la }
            if a.personal != b.personal { return a.personal }
            if rank(a.quality) != rank(b.quality) { return rank(a.quality) < rank(b.quality) }
            if legacy(a) != legacy(b) { return !legacy(a) }
            return a.name.localizedCaseInsensitiveCompare(b.name) == .orderedAscending
        }
    }

    /// The old MacinTalk voices (Fred, Junior, Kathy, Ralph…): installed everywhere, robotic. Last.
    static func legacy(_ voice: SpeechVoiceInfo) -> Bool {
        voice.id.hasPrefix("com.apple.speech.synthesis.voice")
    }

    static func info(_ voice: AVSpeechSynthesisVoice) -> SpeechVoiceInfo {
        let quality: String
        switch voice.quality {
        case .premium: quality = "premium"
        case .enhanced: quality = "enhanced"
        default: quality = "default"
        }
        var personal = false
        if #available(iOS 17.0, macOS 14.0, *) { personal = voice.voiceTraits.contains(.isPersonalVoice) }
        return SpeechVoiceInfo(id: voice.identifier, name: voice.name, language: voice.language, quality: quality, personal: personal)
    }

    /// The best installed voice for the person's language, their own region first ("en-US" before "en-GB").
    static func best() -> SpeechVoiceInfo? {
        let preferred = (Locale.preferredLanguages.first ?? "en-US").replacingOccurrences(of: "_", with: "-")
        let language = String(preferred.prefix(2))
        let voices = all().filter { !$0.personal }
        let rank = { (q: String) in q == "premium" ? 0 : q == "enhanced" ? 1 : 2 }
        let mine = voices.filter { $0.language.hasPrefix(language) }
            .sorted { (rank($0.quality), legacy($0) ? 1 : 0, $0.language == preferred ? 0 : 1) < (rank($1.quality), legacy($1) ? 1 : 0, $1.language == preferred ? 0 : 1) }
        return mine.first ?? voices.first
    }

    /// The chosen voice, or the best one when it's gone (deleted, or never chosen).
    static func resolve(_ id: String?) -> AVSpeechSynthesisVoice? {
        if let id, let voice = AVSpeechSynthesisVoice(identifier: id) { return voice }
        return best().flatMap { AVSpeechSynthesisVoice(identifier: $0.id) }
    }

    /// Everything the voice picker needs from one scan of the installed voices (slow: call it off the
    /// main thread): the voices in order, the best one, and whether a better one is worth getting.
    static func scan() -> (voices: [SpeechVoiceInfo], best: String?, needsBetter: Bool) {
        let voices = all()
        let preferred = (Locale.preferredLanguages.first ?? "en-US").replacingOccurrences(of: "_", with: "-")
        let language = String(preferred.prefix(2))
        let rank = { (q: String) in q == "premium" ? 0 : q == "enhanced" ? 1 : 2 }
        let mine = voices.filter { !$0.personal && $0.language.hasPrefix(language) }
        let key = { (v: SpeechVoiceInfo) in (rank(v.quality), legacy(v) ? 1 : 0, v.language == preferred ? 0 : 1) }
        let best = mine.min { key($0) < key($1) }
        return (voices, (best ?? voices.first { !$0.personal })?.id, !mine.contains { $0.quality != "default" })
    }

    /// True when only default-quality voices are installed for the person's language.
    static var needsBetterVoice: Bool {
        let language = Locale.preferredLanguages.first.map { String($0.prefix(2)) } ?? "en"
        return !all().contains { $0.language.hasPrefix(language) && $0.quality != "default" }
    }

    /// Ask to use the person's Personal Voice; `done(true)` once allowed.
    static func requestPersonalVoice(_ done: @escaping (Bool) -> Void) {
        if #available(iOS 17.0, macOS 14.0, *) {
            AVSpeechSynthesizer.requestPersonalVoiceAuthorization { status in
                DispatchQueue.main.async { done(status == .authorized) }
            }
        } else {
            done(false)
        }
    }
}

/// Where playback stands, for the player.
struct SpeechStatus: Equatable {
    var key: String
    var playing: Bool
    /// Seconds into the audio.
    var position: Double
    /// Seconds rendered so far; the total once `complete`.
    var duration: Double
    var complete: Bool
    /// The sentence being spoken.
    var sentence: Int
    /// Sentences rendered so far, and in all: with `duration`, an estimate of the total while rendering.
    var rendered: Int
    var total: Int
    var failed: String?

    /// The whole length: exact once rendered, estimated before.
    var estimatedTotal: Double {
        if complete || rendered == 0 { return duration }
        return duration * Double(total) / Double(rendered)
    }
}

final class SpeechPlayer {
    /// Called on the main queue whenever the status changes, and several times a second while playing.
    var onChange: ((SpeechStatus) -> Void)?

    private let queue = DispatchQueue(label: "bomb.speech")
    private let engine = AVAudioEngine()
    private let node = AVAudioPlayerNode()
    private let pitch = AVAudioUnitTimePitch()
    private var connectedRate: Double = 0
    private var synthesizer = AVSpeechSynthesizer()
    private var previewer = AVSpeechSynthesizer()
    private var ticker: DispatchSourceTimer?

    // The piece in play.
    private var key = ""
    private var file: URL?
    private var sampleRate: Double = 22050
    private var renderedFrames: AVAudioFramePosition = 0
    private var complete = false
    private var offsets: [Double] = []
    private var totalSentences = 0
    private var generation = 0
    // Playback.
    private var startFrame: AVAudioFramePosition = 0
    private var scheduledFrame: AVAudioFramePosition = 0
    private var queued = 0
    /// Bumped whenever the player node stops, so completions from before don't count.
    private var epoch = 0
    /// Where playback (re)starts on the next feed: after play, seek, resume, or running dry.
    private var restartFrom: AVAudioFramePosition?
    private var playing = false
    private var failed: String?
    private var rate: Float = 1

    /// Frames per scheduled chunk (half a second) and how many to keep queued.
    private let chunkSeconds = 0.5
    private let ahead = 3

    init() {
        engine.attach(node)
        engine.attach(pitch)
        // Tests render and play without making a sound.
        if ProcessInfo.processInfo.environment["BOMB_SPEAK_MUTE"] == "1" { engine.mainMixerNode.outputVolume = 0 }
    }

    // MARK: Control

    /// Read `sentences` aloud from `from` seconds. `key` names the rendering in the cache (message,
    /// voice and text), so playing it again starts at once.
    func play(key: String, sentences: [String], voiceId: String?, rate: Float, from: Double = 0) {
        queue.async { [self] in
            stopLocked()
            generation += 1
            self.key = key
            self.rate = rate
            failed = nil
            let dir = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("bomb-speech", isDirectory: true)
            try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            let audio = dir.appendingPathComponent("\(key).f32")
            let sidecar = dir.appendingPathComponent("\(key).json")
            file = audio
            totalSentences = sentences.count
            if let cached = Self.readSidecar(sidecar), cached.complete, let size = try? FileManager.default.attributesOfItem(atPath: audio.path)[.size] as? Int, size == Int(cached.frames) * 4 {
                sampleRate = cached.sampleRate
                renderedFrames = cached.frames
                offsets = cached.offsets
                complete = true
            } else {
                FileManager.default.createFile(atPath: audio.path, contents: nil)
                renderedFrames = 0
                offsets = []
                complete = false
                render(sentences, voice: SpeechVoices.resolve(voiceId), to: audio, sidecar: sidecar, generation: generation)
            }
            startFrame = AVAudioFramePosition(max(0, from) * sampleRate)
            restartFrom = startFrame
            playing = true
            startEngineLocked()
            feedLocked()
            startTicker()
            publishLocked()
        }
    }

    func pause() {
        queue.async { [self] in
            guard playing else { return }
            startFrame = currentFrameLocked()
            haltLocked()
            playing = false
            stopTicker()
            publishLocked()
        }
    }

    func resume() {
        queue.async { [self] in
            guard !playing, file != nil else { return }
            if complete && startFrame >= renderedFrames { startFrame = 0 }
            restartFrom = startFrame
            playing = true
            startEngineLocked()
            feedLocked()
            startTicker()
            publishLocked()
        }
    }

    func seek(to seconds: Double) {
        queue.async { [self] in
            guard file != nil else { return }
            let target = AVAudioFramePosition(max(0, seconds) * sampleRate)
            // Not rendered yet: go as far as there is.
            startFrame = min(target, renderedFrames)
            haltLocked()
            restartFrom = startFrame
            if playing { feedLocked() }
            publishLocked()
        }
    }

    func skip(by seconds: Double) {
        queue.async { [self] in
            let now = Double(currentFrameLocked()) / sampleRate
            queue.async { self.seek(to: now + seconds) }
        }
    }

    func setRate(_ rate: Float) {
        queue.async { [self] in
            self.rate = rate
            pitch.rate = rate
            publishLocked()
        }
    }

    func stop() {
        queue.async { [self] in
            stopLocked()
            key = ""
            file = nil
            publishLocked()
        }
    }

    /// A short sample in `voiceId`, spoken straight away.
    func preview(voiceId: String, text: String = "Hi, this is how your replies will sound.") {
        previewer.stopSpeaking(at: .immediate)
        let utterance = AVSpeechUtterance(string: text)
        utterance.voice = AVSpeechSynthesisVoice(identifier: voiceId)
        previewer.speak(utterance)
    }

    /// Each sentence's start, in seconds, as rendered so far.
    var sentenceOffsets: [Double] { queue.sync { offsets } }

    // MARK: Rendering

    private func render(_ sentences: [String], voice: AVSpeechSynthesisVoice?, to audio: URL, sidecar: URL, generation: Int) {
        synthesizer = AVSpeechSynthesizer()
        let synthesizer = self.synthesizer
        let writer = try? FileHandle(forWritingTo: audio)
        var index = 0
        var settledRate = false
        func next() {
            guard generation == self.generation else { return }
            guard index < sentences.count else {
                complete = true
                try? writer?.close()
                Self.writeSidecar(sidecar, Sidecar(sampleRate: sampleRate, frames: renderedFrames, complete: true, offsets: offsets))
                feedLocked()
                publishLocked()
                return
            }
            offsets.append(Double(renderedFrames) / sampleRate)
            let utterance = AVSpeechUtterance(string: sentences[index])
            utterance.voice = voice
            index += 1
            var ended = false
            synthesizer.write(utterance) { [weak self] buffer in
                guard let self, let pcm = buffer as? AVAudioPCMBuffer else { return }
                self.queue.async {
                    guard generation == self.generation, !ended else { return }
                    if pcm.frameLength == 0 {
                        // The sentence is done; a short breath, then the next.
                        ended = true
                        self.append(silence: 0.12, to: writer)
                        next()
                        return
                    }
                    if !settledRate {
                        settledRate = true
                        self.sampleRate = pcm.format.sampleRate
                    }
                    self.append(pcm, to: writer)
                    self.feedLocked()
                }
            }
        }
        next()
    }

    private func append(_ pcm: AVAudioPCMBuffer, to writer: FileHandle?) {
        let frames = Int(pcm.frameLength)
        var samples = [Float](repeating: 0, count: frames)
        if let float = pcm.floatChannelData {
            for i in 0..<frames { samples[i] = float[0][i] }
        } else if let int16 = pcm.int16ChannelData {
            for i in 0..<frames { samples[i] = Float(int16[0][i]) / 32768 }
        } else if let int32 = pcm.int32ChannelData {
            for i in 0..<frames { samples[i] = Float(int32[0][i]) / 2_147_483_648 }
        }
        samples.withUnsafeBufferPointer { writer?.write(Data(buffer: $0)) }
        renderedFrames += AVAudioFramePosition(frames)
    }

    private func append(silence seconds: Double, to writer: FileHandle?) {
        let frames = Int(seconds * sampleRate)
        let samples = [Float](repeating: 0, count: frames)
        samples.withUnsafeBufferPointer { writer?.write(Data(buffer: $0)) }
        renderedFrames += AVAudioFramePosition(frames)
    }

    // MARK: Playback

    private var format: AVAudioFormat? { AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 1) }

    private func startEngineLocked() {
        guard let format else { return }
        if connectedRate != sampleRate {
            engine.stop()
            engine.disconnectNodeOutput(node)
            engine.disconnectNodeOutput(pitch)
            engine.connect(node, to: pitch, format: format)
            engine.connect(pitch, to: engine.mainMixerNode, format: format)
            connectedRate = sampleRate
        }
        pitch.rate = rate
        if !engine.isRunning {
            do { try engine.start() } catch { failed = "Audio couldn't start: \(error.localizedDescription)" }
        }
    }

    /// Keep a few chunks queued from the file, as far as it's rendered.
    private func feedLocked() {
        guard playing, let file, let format else { return }
        if let from = restartFrom {
            haltLocked()
            startFrame = from
            scheduledFrame = from
            restartFrom = nil
        }
        let chunk = AVAudioFrameCount(chunkSeconds * sampleRate)
        guard let reader = try? FileHandle(forReadingFrom: file) else { return }
        defer { try? reader.close() }
        while queued < ahead {
            let available = renderedFrames - scheduledFrame
            // Wait for more unless it's the end.
            if available < AVAudioFramePosition(chunk) && !complete { break }
            if available <= 0 { break }
            let frames = AVAudioFrameCount(min(AVAudioFramePosition(chunk), available))
            guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frames) else { break }
            try? reader.seek(toOffset: UInt64(scheduledFrame) * 4)
            let data = (try? reader.read(upToCount: Int(frames) * 4)) ?? Data()
            let got = data.count / 4
            guard got > 0 else { break }
            data.withUnsafeBytes { raw in
                let floats = raw.bindMemory(to: Float.self)
                for i in 0..<got { buffer.floatChannelData![0][i] = floats[i] }
            }
            buffer.frameLength = AVAudioFrameCount(got)
            scheduledFrame += AVAudioFramePosition(got)
            queued += 1
            let epoch = self.epoch
            node.scheduleBuffer(buffer) { [weak self] in
                guard let self else { return }
                self.queue.async {
                    guard epoch == self.epoch, self.playing else { return }
                    self.queued = max(0, self.queued - 1)
                    guard self.queued == 0 else { self.feedLocked(); return }
                    if self.complete && self.scheduledFrame >= self.renderedFrames {
                        // The end.
                        self.startFrame = self.renderedFrames
                        self.haltLocked()
                        self.playing = false
                        self.stopTicker()
                        self.publishLocked()
                    } else {
                        // Ran dry ahead of rendering: carry on from here once there's more.
                        self.restartFrom = self.scheduledFrame
                        self.feedLocked()
                    }
                }
            }
        }
        if !node.isPlaying && queued > 0 { node.play() }
    }

    private func currentFrameLocked() -> AVAudioFramePosition {
        guard playing, node.isPlaying, let nodeTime = node.lastRenderTime, let playerTime = node.playerTime(forNodeTime: nodeTime) else { return startFrame }
        return min(startFrame + playerTime.sampleTime, renderedFrames)
    }

    /// Stop the player node and forget what it had queued.
    private func haltLocked() {
        epoch += 1
        node.stop()
        queued = 0
    }

    private func stopLocked() {
        generation += 1
        synthesizer.stopSpeaking(at: .immediate)
        haltLocked()
        restartFrom = nil
        playing = false
        stopTicker()
    }

    private func stopTicker() {
        ticker?.cancel()
        ticker = nil
    }

    private func startTicker() {
        ticker?.cancel()
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + 0.2, repeating: 0.2)
        timer.setEventHandler { [weak self] in self?.publishLocked() }
        timer.resume()
        ticker = timer
    }

    private func publishLocked() {
        let position = Double(currentFrameLocked()) / sampleRate
        let sentence = max(0, (offsets.lastIndex { $0 <= position + 0.01 }) ?? 0)
        let status = SpeechStatus(key: key, playing: playing, position: position, duration: Double(renderedFrames) / sampleRate,
                                  complete: complete, sentence: sentence, rendered: complete ? totalSentences : max(0, offsets.count - 1),
                                  total: totalSentences, failed: failed)
        let onChange = self.onChange
        DispatchQueue.main.async { onChange?(status) }
    }

    // MARK: Cache

    private struct Sidecar: Codable {
        let sampleRate: Double
        let frames: AVAudioFramePosition
        let complete: Bool
        let offsets: [Double]
    }

    private static func readSidecar(_ url: URL) -> Sidecar? {
        (try? Data(contentsOf: url)).flatMap { try? JSONDecoder().decode(Sidecar.self, from: $0) }
    }

    private static func writeSidecar(_ url: URL, _ sidecar: Sidecar) {
        if let data = try? JSONEncoder().encode(sidecar) { try? data.write(to: url) }
    }

    /// A cache name for a message read in a voice: stable across launches (FNV-1a of the text).
    static func cacheKey(message: String, voiceId: String, sentences: [String]) -> String {
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        for byte in (voiceId + "\u{0}" + sentences.joined(separator: "\n")).utf8 {
            hash ^= UInt64(byte)
            hash = hash &* 0x0000_0100_0000_01b3
        }
        let safe = message.filter { $0.isLetter || $0.isNumber || $0 == "-" }.prefix(48)
        return "\(safe)-\(String(hash, radix: 16))"
    }
}
