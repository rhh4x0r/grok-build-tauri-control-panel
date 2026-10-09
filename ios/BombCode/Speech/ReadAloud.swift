import AVFoundation
import Foundation
import MediaPlayer
import Observation
import Security

/// Reading replies aloud on the iPhone with Fish Audio voices: which message plays, where, how fast,
/// in which voice. One message at a time; starting another replaces it. The engine is
/// `SpeechPlayer`, shared with the Mac.
///
/// The key, voice and speed follow the paired Mac (the first machine with a key): taken when it
/// connects and whenever they change there, and changes made here go back to it. A copy stays in
/// this iPhone's Keychain so reading works while the Mac is away.
@MainActor @Observable
final class ReadAloud {
    static let shared = ReadAloud()

    /// Posted when dictation starts, so reading pauses; and by reading when it starts, so dictation stops.
    static let dictationStarted = Notification.Name("BombDictationStarted")
    static let playbackStarted = Notification.Name("BombPlaybackStarted")

    static let rates: [Float] = [0.75, 1, 1.25, 1.5, 2]

    struct Playing: Equatable {
        let machineId: String
        let threadId: String
        let entry: UInt64
        let title: String
    }

    private(set) var playing: Playing?
    private(set) var status = SpeechStatus(key: "", playing: false, position: 0, duration: 0, complete: false, sentence: 0, rendered: 0, total: 0, failed: nil)
    /// The voice picker: which list ("recommended", "popular", "mine"), the search, what came back.
    private(set) var voices: [SpeechVoiceInfo] = []
    private(set) var voiceList = "recommended"
    private(set) var query = ""
    private(set) var voicesLoading = false
    private(set) var voicesError: String?
    private var search: Task<Void, Never>?
    /// Asked to show the message being read.
    var reveal: UInt64?

    /// The Fish Audio key (Keychain), and whether one is saved.
    private(set) var apiKey: String?
    var hasKey: Bool { apiKey != nil }

    /// The chosen Fish Audio voice, and its name for display.
    private(set) var voiceId: String?
    private(set) var voiceName: String?
    var rate: Float {
        didSet {
            UserDefaults.standard.set(rate, forKey: "readAloud.rate"); player.setRate(rate); updateNowPlaying()
            if !adopting && rate != oldValue { share(rate: rate) }
        }
    }

    /// The paired machines, to share settings with (set by the app).
    var machines: () -> [MachineModel] = { [] }
    /// The machine whose settings this iPhone follows.
    private weak var source: MachineModel?
    /// Its name, for the settings screen.
    var sharedWith: String? { source?.name }
    /// Taking settings from a machine, so they aren't sent straight back.
    private var adopting = false

    private let player = SpeechPlayer()

    private init() {
        // An Apple voice id from before Fish Audio ("com.apple…") isn't a Fish voice: start over.
        let saved = UserDefaults.standard.string(forKey: "readAloud.voice")
        if let saved, !saved.contains(".") {
            voiceId = saved
            voiceName = UserDefaults.standard.string(forKey: "readAloud.voiceName")
        }
        let savedRate = UserDefaults.standard.float(forKey: "readAloud.rate")
        rate = savedRate > 0 ? savedRate : 1
        apiKey = FishKey.load()
        player.onChange = { [weak self] status in
            MainActor.assumeIsolated { self?.receive(status) }
        }
        player.onPreview = { [weak self] voice in
            MainActor.assumeIsolated { self?.previewing = voice }
        }
        NotificationCenter.default.addObserver(forName: Self.dictationStarted, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { if self?.status.playing == true { self?.playPause() } }
        }
        setUpRemoteCommands()
    }

    /// The voice to read with: the chosen one, else Fish Audio's "Sarah".
    var chosenVoice: String { voiceId ?? FishAudio.defaultVoice }
    var chosenVoiceName: String { voiceId == nil ? "Sarah" : (voiceName ?? "Your voice") }

    func choose(_ voice: SpeechVoiceInfo) {
        setVoice(voice.id, name: voice.name)
        share(voice: voice)
    }

    private func setVoice(_ id: String, name: String?) {
        voiceId = id
        voiceName = name
        UserDefaults.standard.set(id, forKey: "readAloud.voice")
        UserDefaults.standard.set(name, forKey: "readAloud.voiceName")
    }

    // MARK: Following the Mac

    /// Take a machine's shared settings (on connect, and when they change there). The first machine
    /// with a key is followed; if none has one and this iPhone does, it's offered to that machine.
    func sync(from machine: MachineModel) async {
        let shared: SpeechSettings
        do {
            guard let answer = try await machine.machine.speechSettings() else { return }
            shared = answer
        } catch {
            #if DEBUG
            if Smoke.enabled { print("smoke: speech settings failed", error) }
            #endif
            return
        }
        #if DEBUG
        if Smoke.enabled { print("smoke: speech settings", shared.apiKey == nil ? "no key" : "key", shared.voiceName ?? "-") }
        #endif
        if let source, source !== machine, source.connected { return }
        if shared.apiKey == nil && source !== machine {
            // The machine has no key: give it this iPhone's, so the two match from now on.
            guard let apiKey, machine.isMac else { return }
            source = machine
            try? await machine.machine.setSpeechSettings(apiKey: apiKey, voice: voiceId, voiceName: voiceName, rate: rate)
            return
        }
        source = machine
        adopting = true
        defer { adopting = false }
        if let key = shared.apiKey {
            if key != apiKey { FishKey.save(key); apiKey = key; refreshVoices() }
        } else if apiKey != nil {
            // Removed on the Mac.
            FishKey.delete()
            apiKey = nil
        }
        if let voice = shared.voice { setVoice(voice, name: shared.voiceName) }
        if let shared = shared.rate, shared > 0, shared != rate { rate = shared }
    }

    /// The machine to send changes to: the one followed, else the first connected Mac.
    private var target: MachineModel? {
        if let source, source.connected { return source }
        let connected = machines().filter(\.connected)
        return connected.first(where: \.isMac) ?? connected.first
    }

    private func share(apiKey: String? = nil, voice: SpeechVoiceInfo? = nil, rate: Float? = nil) {
        guard let machine = target else { return }
        source = machine
        Task { try? await machine.machine.setSpeechSettings(apiKey: apiKey, voice: voice?.id, voiceName: voice?.name, rate: rate) }
    }

    /// A key being checked with Fish Audio, and why the last one wasn't accepted.
    private(set) var checkingKey = false
    private(set) var keyError: String?

    /// Check the key with Fish Audio and save it only once it's accepted. True when saved.
    func saveKey(_ key: String) async -> Bool {
        let key = key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty, !checkingKey else { return false }
        checkingKey = true
        keyError = nil
        defer { checkingKey = false }
        do {
            try await FishAudio.validate(key: key)
        } catch {
            keyError = error.localizedDescription
            return false
        }
        FishKey.save(key)
        apiKey = key
        share(apiKey: key)
        refreshVoices()
        return true
    }

    func removeKey() {
        FishKey.delete()
        apiKey = nil
        share(apiKey: "")
    }

    func showList(_ list: String) {
        guard list != voiceList else { return }
        voiceList = list
        voices = []
        refreshVoices()
    }

    func searchVoices(_ text: String) {
        guard text != query else { return }
        query = text
        refreshVoices()
    }

    /// Look up the voices for the current list and search; a newer lookup replaces this one.
    func refreshVoices() {
        search?.cancel()
        voicesLoading = true
        voicesError = nil
        let (key, list, query) = (apiKey ?? "", voiceList, query)
        search = Task {
            // Typing: wait for a pause before asking.
            if !query.isEmpty { try? await Task.sleep(for: .milliseconds(350)) }
            guard !Task.isCancelled else { return }
            do {
                let found = try await FishAudio.voices(key: key, list: list, query: query)
                guard !Task.isCancelled else { return }
                voices = found
                voicesLoading = false
                #if DEBUG
                if Smoke.enabled { print("smoke: voices", found.count) }
                #endif
            } catch {
                guard !Task.isCancelled else { return }
                voicesError = error.localizedDescription
                voicesLoading = false
            }
        }
    }

    func isReading(threadId: String, entry: UInt64) -> Bool {
        playing?.threadId == threadId && playing?.entry == entry
    }

    /// Read a reply, or stop it when it's the one reading.
    func toggle(machineId: String, threadId: String, entry: UInt64, markdown: String) {
        if isReading(threadId: threadId, entry: entry) { return close() }
        let sentences = speakableSentences(text: markdown)
        guard !sentences.isEmpty else { return }
        let voice = chosenVoice
        playing = Playing(machineId: machineId, threadId: threadId, entry: entry, title: sentences[0])
        guard let apiKey else {
            status = SpeechStatus(key: "", playing: false, position: 0, duration: 0, complete: true, sentence: 0, rendered: 0, total: 0,
                                  failed: "Add your Fish Audio API key in Settings → Voice to read replies aloud.")
            return
        }
        NotificationCenter.default.post(name: Self.playbackStarted, object: nil)
        activateSession()
        player.play(key: SpeechPlayer.cacheKey(message: "\(threadId)-\(entry)", voiceId: voice, sentences: sentences),
                    sentences: sentences, voiceId: voice, apiKey: apiKey, rate: rate)
    }

    func playPause() {
        if status.playing { player.pause() } else { activateSession(); player.resume() }
    }

    func skip(_ seconds: Double) { player.skip(by: seconds) }

    func seek(to seconds: Double) { player.seek(to: seconds) }

    func close() {
        playing = nil
        player.stop()
        MPNowPlayingInfoCenter.default().nowPlayingInfo = nil
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    /// Stop when the thread being read is closed.
    func threadClosed(_ threadId: String) {
        if playing?.threadId == threadId { close() }
    }

    /// The voice whose preview is playing.
    private(set) var previewing: String?

    /// Preview a voice, or stop it when it's the one previewing.
    func togglePreview(_ voice: SpeechVoiceInfo) {
        if previewing == voice.id { return stopPreview() }
        activateSession()
        player.preview(voiceId: voice.id, sample: voice.sample, apiKey: apiKey ?? "")
    }

    func stopPreview() { player.stopPreview() }

    private func receive(_ status: SpeechStatus) {
        guard playing != nil || status.key.isEmpty else { return }
        self.status = status
        updateNowPlaying()
    }

    // MARK: Audio session and the lock screen

    /// Spoken audio: plays with the silent switch on, and carries on briefly in the background.
    private func activateSession() {
        let session = AVAudioSession.sharedInstance()
        try? session.setCategory(.playback, mode: .spokenAudio, options: [])
        try? session.setActive(true)
    }

    private func updateNowPlaying() {
        guard let playing else { return }
        MPNowPlayingInfoCenter.default().nowPlayingInfo = [
            MPMediaItemPropertyTitle: playing.title,
            MPMediaItemPropertyArtist: "Bomb Code",
            MPMediaItemPropertyPlaybackDuration: status.estimatedTotal,
            MPNowPlayingInfoPropertyElapsedPlaybackTime: status.position,
            MPNowPlayingInfoPropertyPlaybackRate: status.playing ? Double(rate) : 0,
        ]
    }

    private func setUpRemoteCommands() {
        let center = MPRemoteCommandCenter.shared()
        center.playCommand.addTarget { [weak self] _ in MainActor.assumeIsolated { if self?.status.playing == false { self?.playPause() } }; return .success }
        center.pauseCommand.addTarget { [weak self] _ in MainActor.assumeIsolated { if self?.status.playing == true { self?.playPause() } }; return .success }
        center.togglePlayPauseCommand.addTarget { [weak self] _ in MainActor.assumeIsolated { self?.playPause() }; return .success }
        center.skipBackwardCommand.preferredIntervals = [15]
        center.skipForwardCommand.preferredIntervals = [15]
        center.skipBackwardCommand.addTarget { [weak self] _ in MainActor.assumeIsolated { self?.skip(-15) }; return .success }
        center.skipForwardCommand.addTarget { [weak self] _ in MainActor.assumeIsolated { self?.skip(15) }; return .success }
        center.changePlaybackPositionCommand.addTarget { [weak self] event in
            guard let event = event as? MPChangePlaybackPositionCommandEvent else { return .commandFailed }
            MainActor.assumeIsolated { self?.seek(to: event.positionTime) }
            return .success
        }
    }
}

/// The Fish Audio API key, in this iPhone's Keychain only.
enum FishKey {
    private static let service = "sh.bombcode.fish-audio"

    static func load() -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecReturnData as String: true,
        ]
        var result: AnyObject?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess, let data = result as? Data else { return nil }
        return String(data: data, encoding: .utf8).flatMap { $0.isEmpty ? nil : $0 }
    }

    static func save(_ key: String) {
        delete()
        let item: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
            kSecValueData as String: Data(key.utf8),
        ]
        SecItemAdd(item as CFDictionary, nil)
    }

    static func delete() {
        SecItemDelete([kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service] as CFDictionary)
    }
}
