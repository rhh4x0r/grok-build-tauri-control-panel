import SwiftUI

/// Start a thread in a project on one of the paired machines.
struct NewThreadSheet: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    let started: (ThreadRef) -> Void

    @State private var machineId: String?
    @State private var projects: [String] = []
    @State private var project: String?
    @State private var choices = ComposerChoices()
    @State private var ownWorktree = true
    @State private var error: String?

    private var machine: MachineModel? { app.machine(id: machineId ?? "") }

    var body: some View {
        NavigationStack {
            Form {
                Section("Where") {
                    Picker("Machine", selection: $machineId) {
                        ForEach(app.machines.filter(\.connected)) { machine in
                            Text(machine.name).tag(Optional(machine.id))
                        }
                    }
                    Picker("Project", selection: $project) {
                        ForEach(projects, id: \.self) { root in Text(projectName(root)).tag(Optional(root)) }
                    }
                    .disabled(projects.isEmpty)
                    Toggle("Own branch", isOn: $ownWorktree)
                }
                if let error { Section { Text(error).foregroundStyle(.red) } }
            }
            .safeAreaInset(edge: .bottom) {
                if let machine, let project {
                    Composer(machine: machine, busy: false, choices: $choices, placeholder: "What should it do?", send: { text, images in
                        await start(machine, project, text, images)
                    }, stop: {})
                    .padding(12)
                }
            }
            .navigationTitle("New thread")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
            .task(id: machineId) { await loadProjects() }
            .onAppear { machineId = machineId ?? app.machines.first(where: \.connected)?.id }
        }
    }

    private func loadProjects() async {
        guard let machine else { return }
        projects = (try? await machine.machine.listProjects()) ?? []
        if project == nil || !projects.contains(project!) { project = projects.first }
        await machine.loadBackends()
        if choices.backend == nil, let first = machine.backends.first {
            choices.backend = first.id
            choices.model = first.defaultModel.isEmpty ? nil : first.defaultModel
        }
    }

    private func start(_ machine: MachineModel, _ project: String, _ text: String, _ images: [ImageUpload]) async -> Bool {
        error = nil
        let new = NewThread(projectRoot: project, backend: choices.backend ?? "grok", model: choices.model, effort: choices.effort,
                            approvalMode: choices.mode, prompt: text, ownWorktree: ownWorktree, images: images)
        do {
            let id = try await machine.machine.startThread(new: new)
            dismiss()
            started(ThreadRef(machineId: machine.id, threadId: id))
            return true
        } catch {
            self.error = describe(error)
            return false
        }
    }
}
