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
        Group {
            if let thread { content(thread) } else { ProgressView() }
        }
        .navigationTitle(thread?.title ?? "Thread")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button("Rename", systemImage: "pencil") { newName = thread?.summary?.label ?? ""; renaming = true }
                    if thread?.turnActive == true {
                        Button("Stop", systemImage: "stop.circle", role: .destructive) { Task { await thread?.stop() } }
                    }
                } label: { Image(systemName: "ellipsis.circle") }
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
    }

    @ViewBuilder
    private func content(_ thread: ThreadModel) -> some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 16) {
                    if thread.hasEarlier {
                        Button("Show earlier messages") { Task { await thread.loadEarlier() } }
                            .font(.footnote).frame(maxWidth: .infinity)
                    }
                    if !thread.loaded && thread.entries.isEmpty {
                        ProgressView().frame(maxWidth: .infinity).padding(.top, 40)
                    }
                    ForEach(thread.rows) { row in
                        TranscriptRowView(row: row, thread: thread).id(row.id)
                    }
                    Color.clear.frame(height: 1).id("bottom")
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
            }
            .scrollDismissesKeyboard(.interactively)
            .defaultScrollAnchor(.bottom)
            .onChange(of: thread.revision) { proxy.scrollTo("bottom", anchor: .bottom) }
        }
        .safeAreaInset(edge: .bottom) {
            VStack(spacing: 8) {
                StatusLine(thread: thread)
                if let error { Text(error).font(.caption).foregroundStyle(.red).frame(maxWidth: .infinity, alignment: .leading) }
                Composer(machine: machine, busy: thread.turnActive, choices: $choices, send: { text, images in
                    await send(thread, text, images)
                }, stop: { await thread.stop() })
            }
            .padding(.horizontal, 12)
            .padding(.bottom, 8)
            .background(.bar)
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
            VStack(alignment: .leading, spacing: 4) {
                Text(presence.label).font(.footnote).foregroundStyle(.secondary).lineLimit(1)
                ProgressView(value: Double(presence.progress)).tint(.orange)
            }
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
