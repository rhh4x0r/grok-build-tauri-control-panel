import AVFoundation
import AudioToolbox
import Foundation
import Speech
#if os(macOS)
import AppKit
#endif

/// On-device dictation with Apple's SpeechAnalyzer (iOS 26 / macOS 26 and later).
///
/// A recorder, as far as the person can tell: a chime, a live sound level while they talk, and
/// the words only once they stop. Shared with the Mac's dictation helper (`tools/bomb-dictate`),
/// so keep it free of app code.
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
    private var results: Task<String, Never>?

    /// Listen until `stop()`, which returns what was said. `onLevel` gets how loud the microphone
    /// is (0…1) on the main queue, several times a second.
    func start(onLevel: @escaping (Float) -> Void) async throws {
        guard Self.isAvailable else { throw Failure.unsupported }
        guard await Self.microphoneAllowed() else { throw Failure.noMicrophone }
        guard let locale = await SpeechTranscriber.supportedLocale(equivalentTo: Locale.current) else { throw Failure.noLanguage }
        let transcriber = SpeechTranscriber(locale: locale, transcriptionOptions: [], reportingOptions: [], attributeOptions: [])
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
                for try await result in transcriber.results where result.isFinal {
                    settled += String(result.text.characters)
                }
            } catch {}
            return settled.trimmingCharacters(in: .whitespacesAndNewlines)
        }

        // The chime plays before the microphone takes over the audio session.
        await Self.chime(starting: true)
        #if os(iOS)
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.record, mode: .measurement, options: .duckOthers)
        try session.setActive(true, options: .notifyOthersOnDeactivation)
        #endif
        let mic = engine.inputNode
        let micFormat = mic.outputFormat(forBus: 0)
        guard let converter = AVAudioConverter(from: micFormat, to: format) else { throw Failure.noFormat }
        mic.installTap(onBus: 0, bufferSize: 4096, format: micFormat) { buffer, _ in
            let level = Self.level(of: buffer)
            DispatchQueue.main.async { onLevel(level) }
            if let converted = Self.convert(buffer, with: converter, to: format) {
                input.yield(AnalyzerInput(buffer: converted))
            }
        }
        engine.prepare()
        try engine.start()
        try await analyzer.start(inputSequence: stream)
    }

    /// Stop listening, and return everything that was said once the last words settle.
    func stop() async -> String {
        engine.stop()
        engine.inputNode.removeTap(onBus: 0)
        input?.finish()
        input = nil
        #if os(iOS)
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        #endif
        await Self.chime(starting: false)
        try? await analyzer?.finalizeAndFinishThroughEndOfInput()
        analyzer = nil
        let text = await results?.value ?? ""
        results = nil
        return text
    }

    /// The system's begin/end recording sounds on iPhone; Tink and Pop on the Mac.
    private static func chime(starting: Bool) async {
        #if os(iOS)
        await withCheckedContinuation { done in
            AudioServicesPlaySystemSoundWithCompletion(starting ? 1113 : 1114) { done.resume() }
        }
        #else
        NSSound(named: starting ? "Tink" : "Pop")?.play()
        try? await Task.sleep(for: .milliseconds(200))
        #endif
    }

    /// Loudness 0…1 from the buffer's RMS, over a 50 dB range.
    private static func level(of buffer: AVAudioPCMBuffer) -> Float {
        guard let samples = buffer.floatChannelData?[0], buffer.frameLength > 0 else { return 0 }
        var sum: Float = 0
        for i in 0..<Int(buffer.frameLength) { sum += samples[i] * samples[i] }
        let rms = (sum / Float(buffer.frameLength)).squareRoot()
        let db = 20 * log10(max(rms, 0.000_01))
        return min(max((db + 50) / 50, 0), 1)
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
