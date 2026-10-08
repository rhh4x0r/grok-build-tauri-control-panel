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
            TextField("", text: $text, prompt: Text(placeholder).foregroundStyle(Theme.textFaint), axis: .vertical)
                .font(Theme.prose)
                .foregroundStyle(Theme.text)
                .tint(Theme.accent)
                .lineLimit(1...6)
                .focused($focused)
                .padding(.horizontal, 4)
                .padding(.top, 2)
            HStack(spacing: 8) {
                PhotosPicker(selection: $picks, maxSelectionCount: 4, matching: .images) {
                    Image(systemName: "photo.on.rectangle")
                        .font(.system(size: 14))
                        .foregroundStyle(Theme.textMuted)
                        .frame(width: 30, height: 30)
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
    }

    private func submit() async {
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
