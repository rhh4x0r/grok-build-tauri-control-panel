import PhotosUI
import SwiftUI

/// What the composer sends with a prompt.
struct ComposerChoices: Equatable {
    var mode = "plan"
    var backend: String?
    var model: String?
    var effort: String?
}

/// The pill at the bottom: text, photos, mode and model, and send or stop.
struct Composer: View {
    let machine: MachineModel
    let busy: Bool
    @Binding var choices: ComposerChoices
    var placeholder = "Message"
    let send: (String, [ImageUpload]) async -> Bool
    let stop: () async -> Void

    @State private var text = ""
    @State private var images: [ImageUpload] = []
    @State private var picks: [PhotosPickerItem] = []
    @State private var sending = false
    /// The running `Dictation` (iOS 26+), while recording.
    @State private var dictation: AnyObject?
    @State private var dictationError: String?
    @State private var recordingSince = Date.now
    /// Recent loudness, newest last, for the sound bars.
    @State private var levels: [Float] = []
    @State private var transcribing = false
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if !images.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack {
                        ForEach(Array(images.enumerated()), id: \.offset) { index, image in
                            if let data = Data(base64Encoded: image.data), let ui = UIImage(data: data) {
                                Image(uiImage: ui).resizable().scaledToFill().frame(width: 56, height: 56).clipShape(RoundedRectangle(cornerRadius: 8))
                                    .overlay(alignment: .topTrailing) {
                                        Button { images.remove(at: index) } label: { Image(systemName: "xmark.circle.fill") }
                                            .foregroundStyle(.white, .black.opacity(0.6)).offset(x: 4, y: -4)
                                    }
                            }
                        }
                    }
                }
            }
            if let dictationError {
                Text(dictationError).font(Theme.caption).foregroundStyle(Theme.danger).padding(.horizontal, 4)
            }
            if dictation != nil || transcribing {
                RecordingStrip(since: recordingSince, levels: levels, transcribing: transcribing)
                    .padding(.horizontal, 4)
                    .padding(.top, 2)
            } else {
                TextField("", text: $text, prompt: Text(placeholder).foregroundStyle(Theme.textFaint), axis: .vertical)
                    .font(Theme.prose)
                    .foregroundStyle(Theme.text)
                    .tint(Theme.accent)
                    .lineLimit(1...6)
                    .focused($focused)
                    .padding(.horizontal, 4)
                    .padding(.top, 2)
            }
            HStack(spacing: 8) {
                PhotosPicker(selection: $picks, maxSelectionCount: 4, matching: .images) {
                    Image(systemName: "photo.on.rectangle")
                        .font(.system(size: 14))
                        .foregroundStyle(Theme.textMuted)
                        .frame(width: 30, height: 30)
                }
                if #available(iOS 26.0, *), Dictation.isAvailable || Smoke.showMic {
                    MicButton(recording: dictation != nil) { Task { await toggleDictation() } }
                        .disabled(transcribing)
                }
                ModeMenu(mode: $choices.mode)
                ModelMenu(machine: machine, choices: $choices)
                Spacer(minLength: 4)
                let empty = text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && images.isEmpty
                if busy && text.isEmpty {
                    Button { Task { await stop() } } label: {
                        Image(systemName: "stop.fill").font(.system(size: 12, weight: .bold))
                            .frame(width: 34, height: 34).background(Theme.solid, in: Circle()).foregroundStyle(Theme.onSolid)
                    }
                } else {
                    Button { Task { await submit() } } label: {
                        Group {
                            if sending { ProgressView().tint(Theme.onSolid) } else { Image(systemName: "arrow.up").font(.system(size: 14, weight: .bold)) }
                        }
                        .frame(width: 34, height: 34)
                        .background(empty ? Theme.bubble : Theme.solid, in: Circle())
                        .foregroundStyle(empty ? Theme.textFaint : Theme.onSolid)
                    }
                    .disabled(sending || empty)
                }
            }
        }
        .padding(12)
        .background(RoundedRectangle(cornerRadius: Theme.composerCorner).fill(Theme.glass))
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: Theme.composerCorner))
        .overlay(RoundedRectangle(cornerRadius: Theme.composerCorner).stroke(Theme.hairline, lineWidth: 1))
        .onChange(of: picks) { _, items in Task { await attach(items) } }
        .onDisappear { Task { await stopDictation() } }
        // Reading a reply aloud stops dictation.
        .onReceive(NotificationCenter.default.publisher(for: ReadAloud.playbackStarted)) { _ in Task { await stopDictation() } }
    }

    /// Tap to record, tap to stop. The words are added after what's typed once recording stops,
    /// and never sent on their own, so a prompt can be dictated in parts and edited in between.
    private func toggleDictation() async {
        guard #available(iOS 26.0, *) else { return }
        if dictation != nil { return await stopDictation() }
        dictationError = nil
        NotificationCenter.default.post(name: ReadAloud.dictationStarted, object: nil)
        levels = []
        recordingSince = .now
        let recorder = Dictation()
        dictation = recorder
        do {
            try await recorder.start { level in
                levels.append(level)
                if levels.count > 40 { levels.removeFirst(levels.count - 40) }
            }
        } catch {
            dictation = nil
            dictationError = describe(error)
        }
    }

    private func stopDictation() async {
        guard #available(iOS 26.0, *), let recorder = dictation as? Dictation else { return }
        dictation = nil
        transcribing = true
        let words = await recorder.stop()
        transcribing = false
        guard !words.isEmpty else { return }
        let gap = text.isEmpty || text.hasSuffix(" ") || text.hasSuffix("\n") ? "" : " "
        text += gap + words
    }

    private func submit() async {
        await stopDictation()
        sending = true
        let sent = await send(text, images)
        sending = false
        if sent { text = ""; images = [] }
    }

    private func attach(_ items: [PhotosPickerItem]) async {
        for item in items {
            guard let data = try? await item.loadTransferable(type: Data.self), let upload = ImageUpload.downscaled(data) else { continue }
            images.append(upload)
        }
        picks = []
    }
}

extension ImageUpload {
    /// A JPEG no larger than 1600 px on its long side, which agents read as well as the original.
    static func downscaled(_ data: Data) -> ImageUpload? {
        guard let image = UIImage(data: data) else { return nil }
        let longest = max(image.size.width, image.size.height)
        let scale = min(1, 1600 / max(longest, 1))
        let size = CGSize(width: image.size.width * scale, height: image.size.height * scale)
        let resized = UIGraphicsImageRenderer(size: size).image { _ in image.draw(in: CGRect(origin: .zero, size: size)) }
        guard let jpeg = resized.jpegData(compressionQuality: 0.8) else { return nil }
        return ImageUpload(mimeType: "image/jpeg", data: jpeg.base64EncodedString(), name: nil)
    }
}

/// Plan, ask or auto. Always-approve is only offered at the desk.
struct ModeMenu: View {
    @Binding var mode: String
    static let modes = [("plan", "Plan", "Plans first and asks before building"), ("ask", "Ask", "Asks before each change"), ("auto", "Auto", "Edits on its own, asks for risky steps")]

    var body: some View {
        Menu {
            Picker("Mode", selection: $mode) {
                ForEach(Self.modes, id: \.0) { id, name, detail in
                    VStack { Text(name); Text(detail) }.tag(id)
                }
            }
        } label: {
            PillLabel {
                Image(systemName: mode == "plan" ? "list.bullet.clipboard" : mode == "ask" ? "hand.raised" : "bolt").font(.system(size: 11))
                Text(Self.modes.first { $0.0 == mode }?.1 ?? mode)
            }
        }
    }
}

struct ModelMenu: View {
    let machine: MachineModel
    @Binding var choices: ComposerChoices

    private var label: String {
        let backend = machine.backends.first { $0.id == choices.backend }
        if let model = choices.model { return backend?.models.first { $0.id == model }?.name ?? model }
        return backend?.name ?? choices.backend?.capitalized ?? "Model"
    }

    var body: some View {
        Menu {
            ForEach(machine.backends, id: \.id) { backend in
                Menu(backend.name) {
                    ForEach(backend.models, id: \.id) { model in
                        Button(model.name) { choices.backend = backend.id; choices.model = model.id }
                    }
                }
            }
            Menu("Effort") {
                ForEach(["low", "medium", "high"], id: \.self) { effort in
                    Button(effort.capitalized) { choices.effort = effort }
                }
            }
        } label: {
            PillLabel {
                if let backend = choices.backend { BrandMark(backend: backend, size: 11) }
                Text(label).lineLimit(1)
            }
        }
        .task { await machine.loadBackends() }
    }
}

/// The microphone while idle; a red stop button while recording.
private struct MicButton: View {
    let recording: Bool
    let toggle: () -> Void

    var body: some View {
        Button(action: toggle) {
            Group {
                if recording {
                    RoundedRectangle(cornerRadius: 2).fill(.white).frame(width: 9, height: 9)
                } else {
                    Image(systemName: "mic").font(.system(size: 14)).foregroundStyle(Theme.textMuted)
                }
            }
            .frame(width: 30, height: 30)
            .background { if recording { Circle().fill(Theme.danger) } }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(recording ? "Stop recording" : "Dictate")
    }
}

/// Where the text goes while recording: a red dot, the time, and the sound coming in.
private struct RecordingStrip: View {
    let since: Date
    let levels: [Float]
    let transcribing: Bool
    @State private var blink = false

    var body: some View {
        HStack(spacing: 10) {
            if transcribing {
                ProgressView().controlSize(.small).tint(Theme.textMuted)
                Text("Transcribing…").font(Theme.sans(15)).foregroundStyle(Theme.textMuted)
            } else {
                Circle().fill(Theme.danger).frame(width: 8, height: 8).opacity(blink ? 0.35 : 1)
                    .animation(.easeInOut(duration: 0.7).repeatForever(autoreverses: true), value: blink)
                    .onAppear { blink = true }
                Text(since, style: .timer).font(Theme.mono(14, .medium)).foregroundStyle(Theme.text).monospacedDigit()
                SoundBars(levels: levels).frame(height: 22)
            }
            Spacer(minLength: 0)
        }
        .frame(minHeight: 28)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(transcribing ? "Transcribing" : "Recording")
    }
}

/// The last few moments of loudness as bars, newest on the right.
private struct SoundBars: View {
    let levels: [Float]
    private let count = 28

    var body: some View {
        let recent = Array(levels.suffix(count))
        let padded = Array(repeating: Float(0), count: max(0, count - recent.count)) + recent
        HStack(alignment: .center, spacing: 3) {
            ForEach(Array(padded.enumerated()), id: \.offset) { _, level in
                // Quiet reads as a calm row of short bars; speech lifts them.
                Capsule().fill(Theme.danger.opacity(level < 0.08 ? 0.35 : 0.9))
                    .frame(width: 3, height: min(22, max(4, CGFloat(level) * 22)))
            }
        }
        .animation(.linear(duration: 0.08), value: levels.count)
    }
}
