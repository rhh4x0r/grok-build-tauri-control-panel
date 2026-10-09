#if DEBUG
import Foundation

/// A tap-free run for checking the app in the simulator, like `BOMB_SMOKE=1` on the Mac:
/// pair from `BOMB_PAIR`, start a mock thread, and open it.
///
///     SIMCTL_CHILD_BOMB_SMOKE=1 SIMCTL_CHILD_BOMB_PAIR='bomb://pair?…' xcrun simctl launch booted com.bombcode.companion
enum Smoke {
    static var enabled: Bool { ProcessInfo.processInfo.environment["BOMB_SMOKE"] == "1" }
    /// Show the dictation button where dictation isn't available (the simulator), to check its layout.
    static var showMic: Bool { enabled && ProcessInfo.processInfo.environment["BOMB_SMOKE_MIC"] == "1" }

    @MainActor
    static func run(_ app: AppModel, open: @escaping (ThreadRef) -> Void) async {
        guard enabled else { return }
        let env = ProcessInfo.processInfo.environment
        if app.machines.isEmpty, let code = env["BOMB_PAIR"] {
            let outcomes = (try? await app.pair(code: code)) ?? []
            print("smoke: paired", outcomes.map { $0.machine?.name ?? $0.error ?? "?" })
        }
        guard let machine = app.machines.first else { return print("smoke: no machine") }
        for _ in 0..<50 where !machine.connected { try? await Task.sleep(for: .milliseconds(200)) }
        print("smoke: link", machine.link)
        guard env["BOMB_SMOKE_LIST"] == nil else { return }
        var projects = (try? await machine.machine.listProjects()) ?? []
        if projects.isEmpty, let made = try? await machine.machine.createProject(name: "smoke") { projects = [made] }
        guard let project = projects.first else { return print("smoke: no project") }
        // BOMB_SMOKE_BACKEND / _MODEL / _PROMPT run a real agent instead of the mock.
        let new = NewThread(projectRoot: project, backend: env["BOMB_SMOKE_BACKEND"] ?? "grok", model: env["BOMB_SMOKE_MODEL"] ?? (env["BOMB_SMOKE_BACKEND"] == nil ? "mock" : nil),
                            effort: nil, approvalMode: env["BOMB_SMOKE_MODE"] ?? "plan",
                            prompt: env["BOMB_SMOKE_PROMPT"] ?? "Hello from the phone. What can you do?", ownWorktree: true, images: [])
        do {
            let id = try await machine.machine.startThread(new: new)
            print("smoke: started", id)
            open(ThreadRef(machineId: machine.id, threadId: id))
        } catch {
            print("smoke: start failed", describe(error))
        }
    }
}
#endif

#if !DEBUG
enum Smoke {
    static let showMic = false
}
#endif
