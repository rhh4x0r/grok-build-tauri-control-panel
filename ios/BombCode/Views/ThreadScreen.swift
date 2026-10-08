import SwiftUI

/// One thread: the transcript, what the agent is doing now, and the composer.
struct ThreadScreen: View {
    let machine: MachineModel
    let threadId: String
    @State private var thread: ThreadModel?
    @State private var choices = ComposerChoices()
    @State private var error: String?
    @State private var renaming = false
    @State private var newName = ""

    var body: some View {
        ZStack {
            if let thread { content(thread) } else { ProgressView().tint(Theme.textMuted) }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background { BombBackground(strength: 0.35) }
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(.hidden, for: .navigationBar)
        .toolbar {
            ToolbarItem(placement: .principal) {
                VStack(spacing: 1) {
                    Text(thread?.title ?? "Thread")
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
                    }
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button("Rename", systemImage: "pencil") { newName = thread?.summary?.label ?? ""; renaming = true }
                    if thread?.turnActive == true {
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
        .alert("Rename thread", isPresented: $renaming) {
            TextField("Name", text: $newName)
            Button("Save") { Task { try? await machine.machine.rename(threadId: threadId, label: newName) } }
            Button("Cancel", role: .cancel) {}
        }
        .onAppear {
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
            .onChange(of: thread.revision) { proxy.scrollTo("bottom", anchor: .bottom) }
        }
        .safeAreaInset(edge: .bottom) {
            VStack(spacing: 8) {
                StatusLine(thread: thread)
                if let error { Text(error).font(Theme.caption).foregroundStyle(Theme.danger).frame(maxWidth: .infinity, alignment: .leading) }
                Composer(machine: machine, busy: thread.turnActive, choices: $choices, send: { text, images in
                    await send(thread, text, images)
                }, stop: { await thread.stop() })
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
