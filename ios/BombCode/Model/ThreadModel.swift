import Foundation
import Observation

/// An open thread: its entries, kept current by patches from the shared reducer.
@MainActor @Observable
final class ThreadModel {
    let id: String
    unowned let machine: MachineModel
    private(set) var entries: [EntryView] = []
    private(set) var presence: PresenceView?
    private(set) var hasEarlier = false
    private(set) var loaded = false
    /// Bumped on every change, so the transcript can follow the newest line.
    private(set) var revision = 0

    init(id: String, machine: MachineModel) {
        self.id = id
        self.machine = machine
    }

    var summary: ThreadSummary? { machine.summary(of: id) }
    var title: String { summary?.label ?? "New thread" }
    var turnActive: Bool { presence?.turnActive ?? summary?.running ?? false }

    func apply(_ patches: [ThreadPatch], presence: PresenceView) {
        for patch in patches {
            switch patch {
            case let .reset(entries, hasEarlier):
                self.entries = entries
                self.hasEarlier = hasEarlier
                loaded = true
            case let .upsert(index, entry):
                let i = Int(index)
                if i == entries.count { entries.append(entry) } else if i < entries.count { entries[i] = entry }
            case let .stream(index, delta):
                let i = Int(index)
                guard i < entries.count, case let .text(text) = entries[i].body else { continue }
                entries[i].body = .text(text: text + delta)
            case let .trim(count):
                entries.removeFirst(min(Int(count), entries.count))
            }
        }
        self.presence = presence
        revision += 1
    }

    /// Refresh the status line's clock while a turn runs.
    func tick() {
        guard let now = machine.machine.presence(threadId: id) else { return }
        presence = now
    }

    func loadEarlier() async {
        try? await machine.machine.loadEarlier(threadId: id)
    }

    func send(_ text: String, options: PromptOptions) async throws {
        try await machine.machine.sendPrompt(threadId: id, text: text, options: options)
    }

    func stop() async {
        try? await machine.machine.cancel(threadId: id)
    }

    /// Returns false when another device answered first.
    func answer(requestId: String, optionId: String) async throws -> Bool {
        !(try await machine.machine.respondApproval(threadId: id, requestId: requestId, optionId: optionId))
    }

    var rows: [TranscriptRow] { TranscriptRow.build(entries) }
}

/// What the transcript draws: entries, with runs of thinking and tool steps folded into one line.
enum TranscriptRow: Identifiable {
    case user(EntryView)
    case agent(EntryView)
    case activity(first: UInt64, items: [EntryView])
    case plan(EntryView)
    case approval(EntryView)
    case line(EntryView)

    var id: UInt64 {
        switch self {
        case let .user(e), let .agent(e), let .plan(e), let .approval(e), let .line(e): e.id
        case let .activity(first, _): first
        }
    }

    static func build(_ entries: [EntryView]) -> [TranscriptRow] {
        var rows: [TranscriptRow] = []
        var run: [EntryView] = []
        func flush() {
            if let first = run.first { rows.append(.activity(first: first.id, items: run)) }
            run = []
        }
        for entry in entries {
            switch (entry.role, entry.body) {
            case (.tool, .tool), (.thought, .text):
                run.append(entry)
                continue
            default:
                flush()
            }
            switch (entry.role, entry.body) {
            case (.you, _): rows.append(.user(entry))
            case (.agent, .text): rows.append(.agent(entry))
            case (_, .plan): rows.append(.plan(entry))
            case (_, .approval): rows.append(.approval(entry))
            default: rows.append(.line(entry))
            }
        }
        flush()
        return rows
    }
}

extension EntryView {
    var text: String {
        if case let .text(text) = body { return text }
        return ""
    }
}
