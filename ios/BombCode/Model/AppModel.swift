import Foundation
import Observation
import UIKit

/// Every paired machine and the combined thread list.
@MainActor @Observable
final class AppModel {
    private(set) var machines: [MachineModel] = []

    init() {
        for paired in MachineStore.load() { machines.append(MachineModel(info: paired)) }
        ReadAloud.shared.machines = { [weak self] in self?.machines ?? [] }
    }

    /// Threads from every machine, newest first.
    var threads: [ThreadSummary] {
        machines.flatMap(\.threads).sorted { $0.updatedAt > $1.updatedAt }
    }

    func machine(id: String) -> MachineModel? {
        machines.first { $0.id == id }
    }

    /// Pair with every machine in a scanned or pasted code.
    func pair(code: String) async throws -> [PairOutcome] {
        var outcomes = try await pairAll(text: code, deviceLabel: UIDevice.current.name)
        for (index, outcome) in outcomes.enumerated() {
            guard let paired = outcome.machine else { continue }
            if !MachineStore.save(paired) {
                outcomes[index].error = "Paired, but this phone couldn’t store the key, so it will be forgotten when the app quits."
            }
            machines.removeAll { $0.id == paired.id }
            machines.append(MachineModel(info: paired))
            PhoneNotifications.shared.noteUsed()
        }
        return outcomes
    }

    /// Remove this phone from the machine, then forget it here even if the machine can't be reached.
    func remove(_ machine: MachineModel) async {
        try? await machine.machine.unpair()
        MachineStore.delete(id: machine.id)
        machines.removeAll { $0.id == machine.id }
    }

    /// Pause connections in the background and reconnect at once on return.
    func setActive(_ active: Bool) {
        for machine in machines { machine.machine.setActive(active: active) }
    }

    func reconnectAll() {
        for machine in machines { machine.machine.reconnectNow() }
    }
}
