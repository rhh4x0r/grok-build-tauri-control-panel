import SwiftUI

/// One thread: the transcript, what the agent is doing now, and the composer.
struct ThreadScreen: View {
    let machine: MachineModel
    let threadId: String
    /// A subagent's name when this is a subagent's transcript, which is read-only.
    var subagent: String? = nil
    @State private var thread: ThreadModel?
    @State private var choices = ComposerChoices()
    @State private var error: String?
    @State private var renaming = false
    @State private var newName = ""
    /// Web servers the thread has running, and the one open in the browser.
    @State private var servers: [ThreadServer] = []
    @State private var preview: PreviewTarget?

    var body: some View {
        ZStack {
            if let thread { content(thread) } else { ProgressView().tint(Theme.textMuted) }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background { BombBackground(strength: 0.35) }
        // The transcript fades out under the bar instead of running into the title.
        .overlay(alignment: .top) {
            LinearGradient(colors: [Theme.bg, Theme.bg.opacity(0)], startPoint: .top, endPoint: .bottom)
                .frame(height: 22)
                .allowsHitTesting(false)
        }
        .navigationBarTitleDisplayMode(.inline)
        // Just the chevron: a "Back" label crowds the title.
        .toolbarRole(.editor)
        .toolbarBackground(Theme.bg, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .toolbar {
            ToolbarItem(placement: .principal) {
                VStack(spacing: 1) {
                    Text(subagent ?? thread?.title ?? "Thread")
                        .font(Theme.sans(15, .semibold))
                        .foregroundStyle(Theme.text)
                        .lineLimit(1)
                    if let summary = thread?.summary {
                        HStack(spacing: 5) {
                            BrandMark(backend: summary.backend, size: 10)
                            Text([projectName(summary.projectRoot), summary.model].filter { !$0.isEmpty }.joined(separator: " · "))
                                .font(Theme.mono(11))
                                .foregroundStyle(Theme.textFaint)
                                .lineLimit(1)
                        }
                    } else if subagent != nil {
                        Text("Subagent").font(Theme.mono(11)).foregroundStyle(Theme.textFaint)
                    }
                }
            }
            if !servers.isEmpty {
                ToolbarItem(placement: .topBarTrailing) {
                    ServersMenu(servers: servers) { preview = $0 }
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    if subagent == nil {
                        Button("Rename", systemImage: "pencil") { newName = thread?.summary?.label ?? ""; renaming = true }
                    }
                    if thread?.turnActive == true && subagent == nil {
                        Button("Stop", systemImage: "stop.circle", role: .destructive) { Task { await thread?.stop() } }
                    }
                } label: {
                    Image(systemName: "ellipsis")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(Theme.text)
                        .frame(width: 32, height: 32)
                        .background(Circle().fill(Theme.bubble))
                }
            }
        }
        // A `localhost` link in a reply opens the Mac's server here, not the phone's own localhost.
        .environment(\.openURL, OpenURLAction { url in
            guard let target = LocalLinks.target(url) else { return .systemAction }
            preview = target
            return .handled
        })
        .fullScreenCover(item: $preview) { target in PreviewBrowser(machine: machine, target: target) }
        // Keep the globe button current while the thread is on screen.
        .task(id: threadId) {
            while !Task.isCancelled {
                if let found = try? await machine.machine.threadServers(threadId: threadId), found != servers { servers = found }
                try? await Task.sleep(for: .seconds(5))
            }
        }
        .alert("Rename thread", isPresented: $renaming) {
            TextField("Name", text: $newName)
            Button("Save") { Task { try? await machine.machine.rename(threadId: threadId, label: newName) } }
            Button("Cancel", role: .cancel) {}
        }
        .onAppear {
            // Reading belongs to the thread it came from; opening another stops it (a subagent of it doesn't).
            if subagent == nil, let playing = ReadAloud.shared.playing, playing.threadId != threadId { ReadAloud.shared.close() }
            let model = machine.open(threadId: threadId)
            thread = model
            if let summary = model.summary {
                choices.mode = ["plan", "ask", "auto"].contains(summary.approvalMode ?? "") ? summary.approvalMode! : "plan"
                choices.backend = summary.backend
                choices.model = summary.model.isEmpty ? nil : summary.model
            }
        }
        .onDisappear { machine.close(threadId: threadId) }
        // A thread opened right after it started has no details yet; take its agent and mode once they arrive.
        .onChange(of: thread?.summary?.backend) { _, backend in
            guard choices.backend == nil, let summary = thread?.summary, backend != nil else { return }
            choices.mode = ["plan", "ask", "auto"].contains(summary.approvalMode ?? "") ? summary.approvalMode! : "plan"
            choices.backend = summary.backend
            choices.model = summary.model.isEmpty ? nil : summary.model
        }
    }

    @ViewBuilder
    private func content(_ thread: ThreadModel) -> some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 18) {
                    if thread.hasEarlier {
                        Button("Show earlier messages") { Task { await thread.loadEarlier() } }
                            .font(Theme.small).foregroundStyle(Theme.textMuted).frame(maxWidth: .infinity)
                    }
                    if !thread.loaded && thread.entries.isEmpty {
                        ProgressView().tint(Theme.textMuted).frame(maxWidth: .infinity).padding(.top, 40)
                    }
                    ForEach(thread.rows) { row in
                        TranscriptRowView(row: row, thread: thread).id(row.id)
                    }
                    Color.clear.frame(height: 1).id("bottom")
                }
                .padding(.horizontal, 18)
                .padding(.vertical, 12)
            }
            .scrollIndicators(.hidden)
            .scrollDismissesKeyboard(.interactively)
            .defaultScrollAnchor(.bottom)
            .onChange(of: thread.revision) { if ReadAloud.shared.reveal == nil { proxy.scrollTo("bottom", anchor: .bottom) } }
            // The player asked to show the message it's reading.
            .onChange(of: ReadAloud.shared.reveal) { _, entry in
                guard let entry else { return }
                withAnimation { proxy.scrollTo(entry, anchor: .top) }
                ReadAloud.shared.reveal = nil
            }
        }
        .safeAreaInset(edge: .bottom) {
            VStack(spacing: 8) {
                if ReadAloud.shared.playing?.threadId == threadId {
                    MiniPlayer()
                }
                StatusLine(thread: thread)
                if let error { Text(error).font(Theme.caption).foregroundStyle(Theme.danger).frame(maxWidth: .infinity, alignment: .leading) }
                if subagent != nil {
                    Label("A subagent's work, read-only. Write to the thread that started it.", systemImage: "arrow.turn.up.left")
                        .font(Theme.caption)
                        .foregroundStyle(Theme.textFaint)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.vertical, 6)
                } else {
                    Composer(machine: machine, busy: thread.turnActive, choices: $choices, send: { text, images in
                        await send(thread, text, images)
                    }, stop: { await thread.stop() })
                }
            }
            .padding(.horizontal, 12)
            .padding(.bottom, 8)
            .padding(.top, 6)
            .background(alignment: .bottom) {
                // Fade the transcript out under the composer instead of a hard toolbar edge.
                LinearGradient(colors: [Theme.bg.opacity(0), Theme.bg.opacity(0.92)], startPoint: .top, endPoint: .center)
                    .ignoresSafeArea()
            }
        }
    }

    private func send(_ thread: ThreadModel, _ text: String, _ images: [ImageUpload]) async -> Bool {
        error = nil
        let options = PromptOptions(backend: choices.backend, model: choices.model, effort: choices.effort, approvalMode: choices.mode, images: images)
        do {
            try await thread.send(text, options: options)
            return true
        } catch {
            self.error = describe(error)
            return false
        }
    }
}

/// "Thinking · 12s" with the thin progress bar, while a turn runs.
struct StatusLine: View {
    let thread: ThreadModel

    var body: some View {
        if let presence = thread.presence, presence.turnActive {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 6) {
                    Circle().fill(Theme.accent).frame(width: 6, height: 6)
                    Text(presence.label).font(Theme.mono(12)).foregroundStyle(Theme.textMuted).lineLimit(1)
                }
                // The fuse: burns from left to right as the turn goes on.
                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        Capsule().fill(Theme.bubble)
                        Capsule().fill(Theme.fuse).frame(width: max(6, geo.size.width * CGFloat(presence.progress)))
                    }
                }
                .frame(height: 2)
            }
            .padding(.horizontal, 6)
            .padding(.top, 6)
            .task(id: thread.revision) {
                while !Task.isCancelled {
                    try? await Task.sleep(for: .seconds(1))
                    thread.tick()
                }
            }
        }
    }
}
