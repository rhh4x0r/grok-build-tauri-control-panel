import AVFoundation
import Foundation

/// Reading replies aloud with Fish Audio voices (fish.audio, with the person's own API key).
///
/// Speech arrives over the network as raw 16-bit audio, a few sentences per request, and is
/// written into a cached audio file (raw 32-bit float, mono); playback starts once the first
/// sentences are in and fetching stays ahead of the playhead, and an `AVAudioEngine` with a
/// time-pitch unit plays the file, so seeking is exact and speed keeps the voice's pitch. Shared
/// with the Mac's helper (`tools/bomb-speak`); keep it free of app code.

/// A Fish Audio voice to read with.
struct SpeechVoiceInfo: Codable, Hashable {
    let id: String
    let name: String
    let author: String
    let languages: [String]
    /// A recording of the voice, for a preview; none when the voice has no samples.
    let sample: String?
    let likes: Int
}

enum FishAudio {
    /// Free on Fish Audio's developer tier.
    static let defaultModel = "s2.1-pro-free"
    static let sampleRate: Double = 44100
    private static let base = URL(string: "https://api.fish.audio")!

    struct Failure: Error, LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    /// Fish Audio's own voices: clean, made for reading.
    static let officialAuthor = "d8b0991f96b44e489422ca2ddf0bd31d"
    /// "Sarah" (Fish Official): read with until another voice is chosen.
    static let defaultVoice = "933563129e564b19a115bedd57b7406a"

    /// The person's language ("en"), for the voice lists.
    static var language: String { Locale.preferredLanguages.first.map { String($0.prefix(2)) } ?? "en" }

    /// Voices for the picker. `list` is "recommended" (Fish Audio's own, in the person's language),
    /// "popular" (everyone's, most used first, matching `query`), or "mine" (the person's own).
    static func voices(key: String, list: String, query: String = "") async throws -> [SpeechVoiceInfo] {
        var url = URLComponents(url: base.appendingPathComponent("model"), resolvingAgainstBaseURL: false)!
        var items = [URLQueryItem(name: "page_size", value: "60")]
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
        switch list {
        case "mine":
            items.append(URLQueryItem(name: "self", value: "true"))
        case "popular":
            items.append(URLQueryItem(name: "sort_by", value: "score"))
            if trimmed.isEmpty { items.append(URLQueryItem(name: "language", value: language)) }
        default:
            items.append(URLQueryItem(name: "author_id", value: officialAuthor))
            items.append(URLQueryItem(name: "language", value: language))
        }
        if !trimmed.isEmpty { items.append(URLQueryItem(name: "title", value: trimmed)) }
        url.queryItems = items
        var request = URLRequest(url: url.url!)
        request.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
        let (data, response) = try await URLSession.shared.data(for: request)
        try check(response, body: data)
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let rows = object["items"] as? [[String: Any]] else { throw Failure(message: "Fish Audio sent an unexpected answer.") }
        return rows.compactMap { row in
            guard let id = row["_id"] as? String, let title = row["title"] as? String else { return nil }
            if let state = row["state"] as? String, state != "trained" { return nil }
            if let type = row["type"] as? String, type != "tts" { return nil }
            let samples = row["samples"] as? [[String: Any]] ?? []
            let author = (row["author"] as? [String: Any])?["nickname"] as? String ?? ""
            return SpeechVoiceInfo(id: id, name: title.trimmingCharacters(in: .whitespacesAndNewlines), author: author,
                                   languages: row["languages"] as? [String] ?? [],
                                   sample: samples.compactMap { $0["audio"] as? String }.first { !$0.isEmpty },
                                   likes: row["like_count"] as? Int ?? 0)
        }
    }

    /// Speech for `text` as it's made: raw 16-bit little-endian mono at `sampleRate`, or `format`.
    static func speech(_ text: String, voice: String?, key: String, model: String, format: String = "pcm") async throws -> URLSession.AsyncBytes {
        var request = URLRequest(url: base.appendingPathComponent("v1/tts"))
        request.httpMethod = "POST"
        request.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue(model, forHTTPHeaderField: "model")
        var body: [String: Any] = ["text": text, "format": format, "latency": "balanced"]
        if format == "pcm" { body["sample_rate"] = Int(sampleRate) }
        if let voice, !voice.isEmpty { body["reference_id"] = voice }
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (bytes, response) = try await URLSession.shared.bytes(for: request)
        if let http = response as? HTTPURLResponse, http.statusCode != 200 {
            var data = Data()
            for try await byte in bytes { data.append(byte); if data.count > 4096 { break } }
            try check(response, body: data)
        }
        return bytes
    }

    /// Check a key with Fish Audio before it's saved: throws, in words, when it's not accepted.
    static func validate(key: String) async throws {
        var request = URLRequest(url: base.appendingPathComponent("wallet/self/api-credit"))
        request.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
        request.timeoutInterval = 15
        let data: Data, response: URLResponse
        do {
            (data, response) = try await URLSession.shared.data(for: request)
        } catch {
            throw Failure(message: "Couldn't reach Fish Audio to check the key: \(error.localizedDescription)")
        }
        if let http = response as? HTTPURLResponse, http.statusCode == 401 || http.statusCode == 403 {
            throw Failure(message: "Fish Audio didn't accept this API key. Copy it again from fish.audio → API Keys.")
        }
        try check(response, body: data)
    }

    /// Throws what went wrong, in words, for anything but a success.
    static func check(_ response: URLResponse, body: Data) throws {
        guard let http = response as? HTTPURLResponse, http.statusCode != 200 else { return }
        let said = (try? JSONSerialization.jsonObject(with: body) as? [String: Any])
            .flatMap { ($0["message"] as? String) ?? ($0["detail"] as? String) }
        switch http.statusCode {
        case 401, 403: throw Failure(message: "Fish Audio didn't accept the API key. Check it in Settings → Voice.")
        case 402: throw Failure(message: "Your Fish Audio account is out of credit.")
        case 429: throw Failure(message: "Fish Audio is busy (rate limited). Try again in a moment.")
        default: throw Failure(message: "Fish Audio: \(said ?? "error \(http.statusCode)")")
        }
    }

    /// Sentences gathered into requests: the first on its own, so reading starts quickly, then a
    /// few hundred characters at a time, so the voice keeps its flow and requests stay few.
    static func chunks(_ sentences: [String]) -> [String] {
        var out: [String] = []
        var current = ""
        for (index, sentence) in sentences.enumerated() {
            if index == 0 { out.append(sentence); continue }
            if !current.isEmpty && current.count + sentence.count > 400 { out.append(current); current = "" }
            current += (current.isEmpty ? "" : " ") + sentence
        }
        if !current.isEmpty { out.append(current) }
        return out
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

/// Its state is only touched on `queue`, which makes it safe to use from any thread.
final class SpeechPlayer: @unchecked Sendable {
    /// Called on the main queue whenever the status changes, and several times a second while playing.
    var onChange: ((SpeechStatus) -> Void)?
    /// Called on the main queue with the voice being previewed, and nil once the preview ends.
    var onPreview: ((String?) -> Void)?

    private let queue = DispatchQueue(label: "bomb.speech")
    private let engine = AVAudioEngine()
    private let node = AVAudioPlayerNode()
    private let pitch = AVAudioUnitTimePitch()
    private var connectedRate: Double = 0
    /// Fetching the piece in play, and the preview playing.
    private var fetch: Task<Void, Never>?
    private var previewer: AVPlayer?
    private var previewTask: Task<Void, Never>?
    private var previewEnd: NSObjectProtocol?
    /// The voice being previewed (touched on the main queue only).
    private(set) var previewing: String?
    private var ticker: DispatchSourceTimer?

    // The piece in play.
    private var key = ""
    private var file: URL?
    private var sampleRate: Double = FishAudio.sampleRate
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
    func play(key: String, sentences: [String], voiceId: String?, apiKey: String, model: String = FishAudio.defaultModel, rate: Float, from: Double = 0) {
        stopPreview()
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
            let chunks = FishAudio.chunks(sentences)
            totalSentences = chunks.count
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
                sampleRate = FishAudio.sampleRate
                render(chunks, voice: voiceId, apiKey: apiKey, model: model, to: audio, sidecar: sidecar, generation: generation)
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

    /// Hear a voice: its sample recording when it has one, else a short line made in it.
    func preview(voiceId: String, sample: String?, apiKey: String, model: String = FishAudio.defaultModel,
                 text: String = "Hi, this is how your replies will sound.") {
        stopPreview()
        if playing { pause() }
        setPreviewing(voiceId)
        if let sample, let url = URL(string: sample) {
            startPreview(url)
            return
        }
        previewTask = Task { [weak self] in
            do {
                var data = Data()
                for try await byte in try await FishAudio.speech(text, voice: voiceId, key: apiKey, model: model, format: "mp3") { data.append(byte) }
                guard !Task.isCancelled else { return }
                let file = FileManager.default.temporaryDirectory.appendingPathComponent("bomb-preview-\(voiceId).mp3")
                try data.write(to: file)
                await MainActor.run { self?.startPreview(file) }
            } catch {
                guard let self, !Task.isCancelled else { return }
                await MainActor.run { self.setPreviewing(nil) }
                queue.async { self.failed = error.localizedDescription; self.publishLocked() }
            }
        }
    }

    /// Play a preview recording; it ends itself when done.
    private func startPreview(_ url: URL) {
        let player = AVPlayer(url: url)
        previewer = player
        previewEnd = NotificationCenter.default.addObserver(forName: .AVPlayerItemDidPlayToEndTime, object: player.currentItem, queue: .main) { [weak self] _ in
            self?.stopPreview()
        }
        player.play()
    }

    /// Stop the preview, if one is playing. Call on the main queue.
    func stopPreview() {
        previewTask?.cancel()
        previewTask = nil
        previewer?.pause()
        previewer = nil
        if let previewEnd { NotificationCenter.default.removeObserver(previewEnd) }
        previewEnd = nil
        setPreviewing(nil)
    }

    private func setPreviewing(_ voice: String?) {
        guard previewing != voice else { return }
        previewing = voice
        onPreview?(voice)
    }

    /// Each sentence's start, in seconds, as rendered so far.
    var sentenceOffsets: [Double] { queue.sync { offsets } }

    // MARK: Rendering

    /// Fetch each chunk in turn, appending its audio to the file as it streams in.
    private func render(_ chunks: [String], voice: String?, apiKey: String, model: String, to audio: URL, sidecar: URL, generation: Int) {
        let writer = try? FileHandle(forWritingTo: audio)
        fetch?.cancel()
        fetch = Task { [weak self] in
            for chunk in chunks {
                guard let self, !Task.isCancelled else { return }
                let current = queue.sync { generation == self.generation }
                guard current else { return }
                queue.sync { self.offsets.append(Double(self.renderedFrames) / self.sampleRate) }
                do {
                    let bytes = try await FishAudio.speech(chunk, voice: voice, key: apiKey, model: model)
                    var pending = Data()
                    pending.reserveCapacity(16384)
                    for try await byte in bytes {
                        pending.append(byte)
                        if pending.count >= 16384 {
                            let ready = pending
                            pending = Data()
                            queue.async { guard generation == self.generation else { return }; self.append(int16: ready, to: writer); self.feedLocked() }
                        }
                        if Task.isCancelled { return }
                    }
                    let rest = pending
                    queue.async {
                        guard generation == self.generation else { return }
                        self.append(int16: rest, to: writer)
                        // A short breath between chunks.
                        self.append(silence: 0.15, to: writer)
                        self.feedLocked()
                    }
                } catch {
                    if Task.isCancelled { return }
                    queue.async {
                        guard generation == self.generation else { return }
                        self.failed = error.localizedDescription
                        // Play what there is, and stop there.
                        self.complete = true
                        try? writer?.close()
                        self.feedLocked()
                        self.finishIfDrainedLocked()
                        self.publishLocked()
                    }
                    return
                }
            }
            guard let self else { return }
            queue.async {
                guard generation == self.generation else { return }
                self.complete = true
                try? writer?.close()
                Self.writeSidecar(sidecar, Sidecar(sampleRate: self.sampleRate, frames: self.renderedFrames, complete: true, offsets: self.offsets))
                self.feedLocked()
                self.finishIfDrainedLocked()
                self.publishLocked()
            }
        }
    }

    /// Append 16-bit little-endian samples (an odd trailing byte is carried to the next call).
    private var carry: UInt8?
    private func append(int16 data: Data, to writer: FileHandle?) {
        var bytes = [UInt8](data)
        if let carry { bytes.insert(carry, at: 0); self.carry = nil }
        if bytes.count % 2 == 1 { carry = bytes.removeLast() }
        let frames = bytes.count / 2
        guard frames > 0 else { return }
        var samples = [Float](repeating: 0, count: frames)
        for i in 0..<frames {
            let value = Int16(bitPattern: UInt16(bytes[2 * i]) | (UInt16(bytes[2 * i + 1]) << 8))
            samples[i] = Float(value) / 32768
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
        guard playing, let file else { return }
        // The voice's own sample rate is known once its first audio arrives (16 kHz for some voices,
        // 22 kHz for most, higher for Premium ones): rebuild the audio path to match before playing.
        if connectedRate != sampleRate {
            haltLocked()
            restartFrom = restartFrom ?? startFrame
            startEngineLocked()
        }
        guard let format else { return }
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

    /// Everything is in and nothing is left queued (it was all played, or there was nothing): stop.
    private func finishIfDrainedLocked() {
        guard playing, complete, queued == 0, scheduledFrame >= renderedFrames else { return }
        startFrame = renderedFrames
        haltLocked()
        playing = false
        stopTicker()
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
        fetch?.cancel()
        fetch = nil
        carry = nil
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
