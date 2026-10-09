import AVFoundation
import Foundation
import MediaPlayer
import Observation

/// Reading replies aloud on the iPhone: which message plays, where, how fast, in which voice. One
/// message at a time; starting another replaces it. The engine is `SpeechPlayer`, shared with the Mac.
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
    private(set) var voices: [SpeechVoiceInfo] = []
    private(set) var needsBetterVoice = false
    var personalVoiceAllowed: Bool?
    /// Asked to show the message being read.
    var reveal: UInt64?

    var voiceId: String? {
        didSet { UserDefaults.standard.set(voiceId, forKey: "readAloud.voice") }
    }
    var rate: Float {
        didSet { UserDefaults.standard.set(rate, forKey: "readAloud.rate"); player.setRate(rate); updateNowPlaying() }
    }

    private let player = SpeechPlayer()

    private init() {
        voiceId = UserDefaults.standard.string(forKey: "readAloud.voice")
        let saved = UserDefaults.standard.float(forKey: "readAloud.rate")
        rate = saved > 0 ? saved : 1
        player.onChange = { [weak self] status in
            MainActor.assumeIsolated { self?.receive(status) }
        }
        refreshVoices()
        NotificationCenter.default.addObserver(forName: Self.dictationStarted, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { if self?.status.playing == true { self?.playPause() } }
        }
        if #available(iOS 17.0, *) {
            NotificationCenter.default.addObserver(forName: AVSpeechSynthesizer.availableVoicesDidChangeNotification, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.refreshVoices() }
            }
        }
        setUpRemoteCommands()
    }

    func refreshVoices() {
        voices = SpeechVoices.all()
        needsBetterVoice = SpeechVoices.needsBetterVoice
    }

    /// The chosen voice while it's installed, else the best one.
    var chosenVoice: String? {
        if let voiceId, voices.contains(where: { $0.id == voiceId }) { return voiceId }
        return SpeechVoices.best()?.id
    }

    func isReading(threadId: String, entry: UInt64) -> Bool {
        playing?.threadId == threadId && playing?.entry == entry
    }

    /// Read a reply, or stop it when it's the one reading.
    func toggle(machineId: String, threadId: String, entry: UInt64, markdown: String) {
        if isReading(threadId: threadId, entry: entry) { return close() }
        let sentences = speakableSentences(text: markdown)
        guard !sentences.isEmpty else { return }
        let voice = chosenVoice ?? "default"
        NotificationCenter.default.post(name: Self.playbackStarted, object: nil)
        activateSession()
        playing = Playing(machineId: machineId, threadId: threadId, entry: entry, title: sentences[0])
        player.play(key: SpeechPlayer.cacheKey(message: "\(threadId)-\(entry)", voiceId: voice, sentences: sentences),
                    sentences: sentences, voiceId: chosenVoice, rate: rate)
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

    func preview(_ voice: String) { player.preview(voiceId: voice) }

    func requestPersonalVoice() {
        SpeechVoices.requestPersonalVoice { [weak self] granted in
            MainActor.assumeIsolated {
                self?.personalVoiceAllowed = granted
                self?.refreshVoices()
            }
        }
    }

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
