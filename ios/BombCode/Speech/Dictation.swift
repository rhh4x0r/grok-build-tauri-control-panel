import AVFoundation
import Foundation
import Speech

/// On-device dictation with Apple's SpeechAnalyzer (iOS 26 / macOS 26 and later).
///
/// Start listening and text arrives as it's recognized: what's settled (`final`), and the words
/// still being worked out (`volatile`), which may change. Stop, and the rest is settled. Shared
/// with the Mac's dictation helper (`tools/bomb-dictate`), so keep it free of app code.
@available(iOS 26.0, macOS 26.0, *)
final class Dictation {
    enum Failure: LocalizedError {
        case unsupported, noLanguage, noMicrophone, noFormat

        var errorDescription: String? {
            switch self {
            case .unsupported: "Dictation isn't available on this device."
            case .noLanguage: "Dictation doesn't support your language yet."
            case .noMicrophone: "Bomb Code needs microphone access to dictate. You can allow it in Settings."
            case .noFormat: "The microphone couldn't be set up for dictation."
            }
        }
    }

    /// Whether this device can dictate at all.
    static var isAvailable: Bool { SpeechTranscriber.isAvailable }

    private let engine = AVAudioEngine()
    private var analyzer: SpeechAnalyzer?
    private var input: AsyncStream<AnalyzerInput>.Continuation?
    private var results: Task<Void, Never>?

    /// Listen until `stop()`. `onText(final, volatile)` is called on the main queue as words arrive;
    /// `final` grows, `volatile` is replaced each time.
    func start(onText: @escaping (String, String) -> Void) async throws {
        guard Self.isAvailable else { throw Failure.unsupported }
        guard await Self.microphoneAllowed() else { throw Failure.noMicrophone }
        guard let locale = await SpeechTranscriber.supportedLocale(equivalentTo: Locale.current) else { throw Failure.noLanguage }
        let transcriber = SpeechTranscriber(locale: locale, transcriptionOptions: [], reportingOptions: [.volatileResults], attributeOptions: [])
        // The language model downloads once, the first time.
        if let download = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) {
            try await download.downloadAndInstall()
        }
        guard let format = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [transcriber]) else { throw Failure.noFormat }
        let analyzer = SpeechAnalyzer(modules: [transcriber])
        let (stream, input) = AsyncStream.makeStream(of: AnalyzerInput.self)
        self.analyzer = analyzer
        self.input = input

        results = Task {
            var settled = ""
            do {
                for try await result in transcriber.results {
                    let words = String(result.text.characters)
                    if result.isFinal { settled += words }
                    let (final, volatile) = (settled, result.isFinal ? "" : words)
                    await MainActor.run { onText(final, volatile) }
                }
            } catch {}
        }

        #if os(iOS)
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.record, mode: .measurement, options: .duckOthers)
        try session.setActive(true, options: .notifyOthersOnDeactivation)
        #endif
        let mic = engine.inputNode
        let micFormat = mic.outputFormat(forBus: 0)
        guard let converter = AVAudioConverter(from: micFormat, to: format) else { throw Failure.noFormat }
        mic.installTap(onBus: 0, bufferSize: 4096, format: micFormat) { buffer, _ in
            if let converted = Self.convert(buffer, with: converter, to: format) {
                input.yield(AnalyzerInput(buffer: converted))
            }
        }
        engine.prepare()
        try engine.start()
        try await analyzer.start(inputSequence: stream)
    }

    /// Stop listening and wait for the last words to settle.
    func stop() async {
        engine.stop()
        engine.inputNode.removeTap(onBus: 0)
        input?.finish()
        input = nil
        try? await analyzer?.finalizeAndFinishThroughEndOfInput()
        analyzer = nil
        await results?.value
        results = nil
        #if os(iOS)
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        #endif
    }

    private static func microphoneAllowed() async -> Bool {
        #if os(iOS)
        await AVAudioApplication.requestRecordPermission()
        #else
        await AVCaptureDevice.requestAccess(for: .audio)
        #endif
    }

    /// The microphone's buffer in the format the analyzer wants.
    private static func convert(_ buffer: AVAudioPCMBuffer, with converter: AVAudioConverter, to format: AVAudioFormat) -> AVAudioPCMBuffer? {
        let ratio = format.sampleRate / buffer.format.sampleRate
        let capacity = AVAudioFrameCount((Double(buffer.frameLength) * ratio).rounded(.up)) + 1
        guard let out = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: capacity) else { return nil }
        var supplied = false
        var error: NSError?
        converter.convert(to: out, error: &error) { _, status in
            if supplied {
                status.pointee = .noDataNow
                return nil
            }
            supplied = true
            status.pointee = .haveData
            return buffer
        }
        return error == nil && out.frameLength > 0 ? out : nil
    }
}
