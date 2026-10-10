import AVKit
import SwiftUI

/// A reply: its text, then the pictures and videos it shows from the machine.
struct AgentReply: View {
    let entry: EntryView
    let thread: ThreadModel

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if !entry.text.isEmpty {
                ReplyText(source: entry.text).padding(.horizontal, 2)
            }
            ForEach(entry.media, id: \.path) { media in
                if media.isVideo {
                    VideoTile(media: media, thread: thread)
                } else {
                    PictureTile(media: media, thread: thread)
                }
            }
            if !entry.streaming && !entry.text.isEmpty && thread.finalReplyIds.contains(entry.id) {
                ReplyFooter(entry: entry, thread: thread)
            }
        }
    }
}

/// Under a finished reply: when it came, copy, and read aloud.
struct ReplyFooter: View {
    let entry: EntryView
    let thread: ThreadModel
    private let reader = ReadAloud.shared

    var body: some View {
        let reading = reader.isReading(threadId: thread.id, entry: entry.id)
        HStack(spacing: 14) {
            Text(Date(timeIntervalSince1970: TimeInterval(entry.atMs) / 1000), style: .time)
                .font(Theme.caption)
                .foregroundStyle(Theme.textFaint)
            CopyIcon(text: entry.text)
            Button {
                reader.toggle(machineId: thread.machine.id, threadId: thread.id, entry: entry.id, markdown: entry.text)
            } label: {
                Image(systemName: reading ? "waveform" : "speaker.wave.2")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(reading ? Theme.accent : Theme.textFaint)
                    .symbolEffect(.variableColor.iterative, isActive: reading && reader.status.playing)
                    .frame(width: 24, height: 24)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(reading ? "Stop reading" : "Read aloud")
            Spacer()
        }
        .padding(.horizontal, 2)
    }
}

/// A copy icon that ticks once it has copied.
struct CopyIcon: View {
    let text: String
    @State private var copied = false

    var body: some View {
        Button {
            UIPasteboard.general.string = text
            copied = true
            Task { try? await Task.sleep(for: .seconds(1.5)); copied = false }
        } label: {
            Image(systemName: copied ? "checkmark" : "doc.on.doc")
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(copied ? Theme.success : Theme.textFaint)
                .frame(width: 24, height: 24)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Copy")
    }
}

/// Fetches a file from the machine once per screen; the core keeps a copy for next time.
@MainActor @Observable
final class MediaLoader {
    enum State { case idle, loading, ready(URL), failed(String) }
    private(set) var state: State = .idle

    func load(_ media: MediaRef, thread: ThreadModel) async {
        if case .loading = state { return }
        if case .ready = state { return }
        state = .loading
        do {
            let path = try await thread.machine.machine.fetchMedia(threadId: thread.id, path: media.path)
            state = .ready(URL(fileURLWithPath: path))
        } catch {
            state = .failed(describe(error))
        }
    }
}

/// A picture, loaded as it scrolls into view; tap for full screen.
struct PictureTile: View {
    let media: MediaRef
    let thread: ThreadModel
    @State private var loader = MediaLoader()
    @State private var image: UIImage?
    @State private var full = false

    var body: some View {
        Group {
            if let image {
                Image(uiImage: image)
                    .resizable()
                    .scaledToFit()
                    .frame(maxWidth: .infinity, maxHeight: 280, alignment: .leading)
                    .clipShape(RoundedRectangle(cornerRadius: Theme.panelCorner))
                    .overlay(RoundedRectangle(cornerRadius: Theme.panelCorner).stroke(Theme.hairline, lineWidth: 1))
                    .onTapGesture { full = true }
            } else {
                placeholder
            }
        }
        .task {
            await loader.load(media, thread: thread)
            if case let .ready(url) = loader.state { image = UIImage(contentsOfFile: url.path) }
        }
        .fullScreenCover(isPresented: $full) {
            if let image { PictureViewer(image: image, title: media.title) }
        }
    }

    private var placeholder: some View {
        RoundedRectangle(cornerRadius: Theme.panelCorner)
            .fill(Theme.bubble)
            .frame(height: 160)
            .overlay {
                switch loader.state {
                case let .failed(message):
                    MediaNote(icon: "photo", title: media.title, detail: message)
                default:
                    ProgressView().tint(Theme.textMuted)
                }
            }
    }
}

/// A video as a card; tap to fetch it from the machine and play it.
struct VideoTile: View {
    let media: MediaRef
    let thread: ThreadModel
    @State private var loader = MediaLoader()
    @State private var playing: URL?

    var body: some View {
        Button {
            Task {
                await loader.load(media, thread: thread)
                if case let .ready(url) = loader.state { playing = url }
            }
        } label: {
            HStack(spacing: 12) {
                ZStack {
                    Circle().fill(Theme.solid).frame(width: 40, height: 40)
                    if case .loading = loader.state {
                        ProgressView().tint(Theme.onSolid)
                    } else {
                        Image(systemName: "play.fill").font(.system(size: 15)).foregroundStyle(Theme.onSolid).offset(x: 1)
                    }
                }
                VStack(alignment: .leading, spacing: 3) {
                    Text(media.title).font(Theme.sans(15, .medium)).foregroundStyle(Theme.text).lineLimit(1)
                    Text(detail).font(Theme.mono(11)).foregroundStyle(Theme.textFaint).lineLimit(2)
                }
                Spacer(minLength: 0)
            }
            .padding(12)
            .background(RoundedRectangle(cornerRadius: Theme.panelCorner).fill(Theme.glass))
            .overlay(RoundedRectangle(cornerRadius: Theme.panelCorner).stroke(Theme.hairline, lineWidth: 1))
        }
        .buttonStyle(.plain)
        .fullScreenCover(item: $playing) { url in VideoViewer(url: url) }
    }

    private var detail: String {
        switch loader.state {
        case .loading: "Getting it from \(thread.machine.name)…"
        case let .failed(message): message
        default: "Video · tap to play"
        }
    }
}

extension URL: @retroactive Identifiable {
    public var id: String { absoluteString }
}

/// A picture that couldn't be shown, and why.
private struct MediaNote: View {
    let icon: String
    let title: String
    let detail: String

    var body: some View {
        VStack(spacing: 6) {
            Image(systemName: icon).font(.system(size: 18)).foregroundStyle(Theme.textFaint)
            Text(title).font(Theme.sans(13, .medium)).foregroundStyle(Theme.textMuted).lineLimit(1)
            Text(detail).font(Theme.caption).foregroundStyle(Theme.textFaint).multilineTextAlignment(.center).lineLimit(2)
        }
        .padding(12)
    }
}

/// Full screen, pinch to zoom, swipe or tap Done to close.
private struct PictureViewer: View {
    let image: UIImage
    let title: String
    @Environment(\.dismiss) private var dismiss
    @State private var scale: CGFloat = 1
    @State private var base: CGFloat = 1

    var body: some View {
        ZStack(alignment: .topTrailing) {
            Color.black.ignoresSafeArea()
            Image(uiImage: image)
                .resizable()
                .scaledToFit()
                .scaleEffect(scale)
                .gesture(MagnifyGesture()
                    .onChanged { scale = max(1, base * $0.magnification) }
                    .onEnded { _ in base = scale })
                .onTapGesture(count: 2) { withAnimation { scale = scale > 1 ? 1 : 2.5; base = scale } }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            HStack {
                ShareLink(item: Image(uiImage: image), preview: SharePreview(title, image: Image(uiImage: image))) {
                    Image(systemName: "square.and.arrow.up")
                }
                Button("Done") { dismiss() }
            }
            .font(Theme.sans(15, .medium))
            .foregroundStyle(.white)
            .padding(.horizontal, 18)
            .padding(.top, 8)
        }
        .gesture(DragGesture().onEnded { if $0.translation.height > 120 && scale == 1 { dismiss() } })
    }
}

/// The system player, full screen.
private struct VideoViewer: View {
    let url: URL
    @Environment(\.dismiss) private var dismiss
    @State private var player: AVPlayer?

    var body: some View {
        ZStack(alignment: .topTrailing) {
            Color.black.ignoresSafeArea()
            if let player {
                VideoPlayer(player: player).ignoresSafeArea(edges: .bottom)
            }
            Button("Done") { player?.pause(); dismiss() }
                .font(Theme.sans(15, .medium))
                .foregroundStyle(.white)
                .padding(.horizontal, 18)
                .padding(.top, 8)
        }
        .onAppear {
            let player = AVPlayer(url: url)
            self.player = player
            player.play()
        }
    }
}
