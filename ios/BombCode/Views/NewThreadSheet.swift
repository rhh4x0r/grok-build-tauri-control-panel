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
        VStack(spacing: 0) {
            SheetHeader(title: "New thread") {
                Button("Cancel") { dismiss() }
            } trailing: { EmptyView() }
            ScrollView {
                VStack(alignment: .leading, spacing: 10) {
                    SectionLabel("Where")
                    VStack(spacing: 0) {
                        PickRow(icon: machine?.isMac == false ? "server.rack" : "laptopcomputer", label: "Machine",
                                value: machine?.name ?? "None online") {
                            ForEach(app.machines.filter(\.connected)) { m in
                                Button(m.name) { machineId = m.id }
                            }
                        }
                        divider
                        PickRow(icon: "folder", label: "Project", value: project.map(projectName) ?? (projects.isEmpty ? "No projects" : "Choose")) {
                            ForEach(projects, id: \.self) { root in
                                Button(projectName(root)) { project = root }
                            }
                        }
                        .disabled(projects.isEmpty)
                        divider
                        HStack(spacing: 12) {
                            Image(systemName: "arrow.triangle.branch").font(.system(size: 14)).foregroundStyle(Theme.textMuted).frame(width: 22)
                            Text("Own branch").font(Theme.sans(15)).foregroundStyle(Theme.text)
                            Spacer()
                            Toggle("", isOn: $ownWorktree).labelsHidden().tint(Theme.accent)
                        }
                        .padding(.horizontal, 12)
                        .frame(minHeight: 50)
                    }
                    .background(RoundedRectangle(cornerRadius: Theme.panelCorner).fill(Theme.glass))
                    .overlay(RoundedRectangle(cornerRadius: Theme.panelCorner).stroke(Theme.hairline, lineWidth: 1))
                    Footnote("Own branch gives the thread its own worktree, so it can't step on your other work.")
                    if let error {
                        Text(error).font(Theme.small).foregroundStyle(Theme.danger).padding(.horizontal, 4)
                    }
                }
                .padding(.horizontal, 16)
            }
            .scrollIndicators(.hidden)
            if let machine, let project {
                Composer(machine: machine, busy: false, choices: $choices, placeholder: "What should it do?", send: { text, images in
                    await start(machine, project, text, images)
                }, stop: {})
                .padding(.horizontal, 12)
                .padding(.bottom, 8)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background { BombBackground(strength: 0.5) }
        .presentationDragIndicator(.visible)
        .task(id: machineId) { await loadProjects() }
        .onAppear { machineId = machineId ?? defaultMachine()?.id }
    }

    private var divider: some View {
        Rectangle().fill(Theme.hairline).frame(height: 1).padding(.leading, 46)
    }

    /// Where the person last worked: the machine of the most recent real thread, a Mac before a server.
    private func defaultMachine() -> MachineModel? {
        let online = app.machines.filter(\.connected)
        let recent = online.compactMap { m in m.threads.filter { $0.model != "mock" }.map(\.updatedAt).max().map { (m, $0) } }
        return recent.max { $0.1 < $1.1 }?.0 ?? online.first(where: \.isMac) ?? online.first
    }

    private func loadProjects() async {
        guard let machine else { return }
        // The machine's projects, plus any folder its threads ran in, most recently used first.
        let listed = (try? await machine.machine.listProjects()) ?? []
        var byRecent: [String] = []
        for thread in machine.threads.sorted(by: { $0.updatedAt > $1.updatedAt }) where !byRecent.contains(thread.projectRoot) {
            byRecent.append(thread.projectRoot)
        }
        projects = byRecent + listed.filter { !byRecent.contains($0) }
        if project == nil || !projects.contains(project!) { project = projects.first }
        await machine.loadBackends()
        // A different machine may not have the agent picked on the last one.
        if let backend = choices.backend, !machine.backends.contains(where: { $0.id == backend }) {
            choices.backend = nil
            choices.model = nil
        }
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

/// A settings-style row that opens a menu of choices.
private struct PickRow<Choices: View>: View {
    let icon: String
    let label: String
    let value: String
    @ViewBuilder var choices: Choices

    var body: some View {
        Menu { choices } label: {
            HStack(spacing: 12) {
                Image(systemName: icon).font(.system(size: 14)).foregroundStyle(Theme.textMuted).frame(width: 22)
                Text(label).font(Theme.sans(15)).foregroundStyle(Theme.text)
                Spacer(minLength: 8)
                Text(value).font(Theme.sans(15)).foregroundStyle(Theme.textMuted).lineLimit(1)
                Image(systemName: "chevron.up.chevron.down").font(.system(size: 11, weight: .medium)).foregroundStyle(Theme.textFaint)
            }
            .padding(.horizontal, 12)
            .frame(minHeight: 50)
            .contentShape(Rectangle())
        }
    }
}
