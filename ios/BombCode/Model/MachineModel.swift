import Foundation
import Observation

/// One paired Mac or server, as the screens see it.
@MainActor @Observable
final class MachineModel: Identifiable {
    let info: PairedMachine
    let machine: Machine
    private(set) var link: LinkState = .connecting
    private(set) var threads: [ThreadSummary] = []
    /// Backends and models this machine offers, loaded once connected.
    private(set) var backends: [BackendChoice] = []
    /// What the machine's own sidebar hides and pins, so the phone lists threads the same way.
    private(set) var prefs = SidebarPrefs(archived: [], pinnedProjects: [])
    private var open: [String: ThreadModel] = [:]

    nonisolated var id: String { info.id }
    var name: String { info.name }
    var isMac: Bool { info.kind == "mac" }
    var connected: Bool { link == .connected }

    init(info: PairedMachine) {
        self.info = info
        let relay = ListenerRelay()
        machine = Machine(machine: info, listener: relay)
        // Callbacks hop to the main queue, so they always find this set.
        relay.owner = self
    }

    /// The model for a thread on screen; starts streaming it.
    func open(threadId: String) -> ThreadModel {
        if let existing = open[threadId] { return existing }
        let model = ThreadModel(id: threadId, machine: self)
        open[threadId] = model
        Task { try? await machine.openThread(threadId: threadId) }
        return model
    }

    func close(threadId: String) {
        guard open.removeValue(forKey: threadId) != nil else { return }
        Task { try? await machine.closeThread(threadId: threadId) }
    }

    func summary(of threadId: String) -> ThreadSummary? {
        threads.first { $0.id == threadId }
    }

    func loadBackends() async {
        guard backends.isEmpty, let list = try? await machine.listBackends() else { return }
        backends = list.filter(\.available)
    }

    func loadPrefs() async {
        if let prefs = try? await machine.sidebarPrefs() { self.prefs = prefs }
    }

    func isArchived(_ thread: ThreadSummary) -> Bool { prefs.archived.contains(thread.id) }

    fileprivate func receive(link: LinkState) {
        self.link = link
        if link == .connected { Task { await loadBackends(); await loadPrefs() } }
    }

    fileprivate func receive(threads: [ThreadSummary]) {
        // A thread came or went: the Mac may have archived or pinned something too.
        let changed = Set(threads.map(\.id)) != Set(self.threads.map(\.id))
        self.threads = threads
        if changed { Task { await loadPrefs() } }
    }

    fileprivate func receive(threadId: String, patches: [ThreadPatch], presence: PresenceView) {
        open[threadId]?.apply(patches, presence: presence)
    }
}

/// Carries callbacks from the Rust side's threads onto the main thread, in the order they were sent.
private final class ListenerRelay: MachineListener, @unchecked Sendable {
    @MainActor weak var owner: MachineModel?

    private func onMain(_ body: @escaping @MainActor (MachineModel) -> Void) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                if let owner = self.owner { body(owner) }
            }
        }
    }

    func onLink(state: LinkState) {
        onMain { $0.receive(link: state) }
    }

    func onThreads(threads: [ThreadSummary]) {
        onMain { $0.receive(threads: threads) }
    }

    func onThread(threadId: String, patches: [ThreadPatch], presence: PresenceView) {
        onMain { $0.receive(threadId: threadId, patches: patches, presence: presence) }
    }
}
