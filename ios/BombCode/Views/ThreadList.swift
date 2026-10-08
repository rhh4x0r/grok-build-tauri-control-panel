import SwiftUI

/// Where a navigation link leads.
struct ThreadRef: Hashable {
    let machineId: String
    let threadId: String
}

/// Every thread on every paired machine, grouped by project.
struct ThreadList: View {
    @Environment(AppModel.self) private var app
    @Binding var path: [ThreadRef]
    @State private var search = ""
    @State private var showMachines = false
    @State private var showNew = false

    private var groups: [(project: String, threads: [ThreadSummary])] {
        let wanted = app.threads.filter { search.isEmpty || ($0.label ?? "").localizedCaseInsensitiveContains(search) || $0.projectRoot.localizedCaseInsensitiveContains(search) }
        var order: [String] = []
        var byProject: [String: [ThreadSummary]] = [:]
        for thread in wanted {
            let key = "\(thread.machineId)\u{0}\(thread.projectRoot)"
            if byProject[key] == nil { order.append(key) }
            byProject[key, default: []].append(thread)
        }
        return order.map { ($0, byProject[$0] ?? []) }
    }

    var body: some View {
        Group {
            if app.machines.isEmpty {
                ContentUnavailableView {
                    Label("Pair with your Mac", systemImage: "laptopcomputer.and.iphone")
                } description: {
                    Text("Open Bomb Code on your Mac, go to Settings → Phone, and scan the code it shows.")
                } actions: {
                    Button("Scan pairing code") { showMachines = true }.buttonStyle(.borderedProminent)
                }
            } else {
                List {
                    ForEach(groups, id: \.project) { group in
                        Section {
                            ForEach(group.threads, id: \.id) { thread in
                                NavigationLink(value: ThreadRef(machineId: thread.machineId, threadId: thread.id)) {
                                    ThreadRow(thread: thread, machine: app.machine(id: thread.machineId))
                                }
                                .swipeActions {
                                    if thread.running {
                                        Button("Stop", role: .destructive) {
                                            Task { try? await app.machine(id: thread.machineId)?.machine.cancel(threadId: thread.id) }
                                        }
                                    }
                                }
                            }
                        } header: {
                            ProjectHeader(root: group.threads.first?.projectRoot ?? "", machine: app.machine(id: group.threads.first?.machineId ?? ""))
                        }
                    }
                }
                .listStyle(.insetGrouped)
                .searchable(text: $search)
                .refreshable {
                    app.reconnectAll()
                    for machine in app.machines { _ = try? await machine.machine.refreshThreads() }
                }
                .overlay {
                    if app.threads.isEmpty { ContentUnavailableView("No threads yet", systemImage: "bubble.left.and.text.bubble.right", description: Text("Start one with the pencil button.")) }
                }
            }
        }
        .navigationTitle("Threads")
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button { showMachines = true } label: { MachinesBadge() }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button { showNew = true } label: { Image(systemName: "square.and.pencil") }
                    .disabled(!app.machines.contains { $0.connected })
            }
        }
        .sheet(isPresented: $showMachines) { MachinesView() }
        .sheet(isPresented: $showNew) {
            NewThreadSheet { ref in path.append(ref) }
        }
    }
}

private struct ThreadRow: View {
    let thread: ThreadSummary
    let machine: MachineModel?

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Circle().fill(Theme.statusColor(thread)).frame(width: 8, height: 8).padding(.top, 6)
            VStack(alignment: .leading, spacing: 3) {
                Text(thread.label ?? "New thread").lineLimit(1)
                HStack(spacing: 4) {
                    if thread.needsApproval { Text("Needs you").foregroundStyle(.orange) }
                    else if thread.running { Text("Working") }
                    Text(timeAgo(thread.updatedAt))
                    Text("·")
                    Text(thread.backend)
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
        }
    }
}

private struct ProjectHeader: View {
    let root: String
    let machine: MachineModel?

    var body: some View {
        HStack(spacing: 6) {
            Text(projectName(root))
            if let machine {
                Label(machine.name, systemImage: machine.isMac ? "laptopcomputer" : "server.rack")
                    .labelStyle(.titleAndIcon)
                    .font(.caption2)
                    .padding(.horizontal, 6).padding(.vertical, 2)
                    .background(Theme.bubble, in: Capsule())
            }
        }
    }
}

/// How many machines are connected, at a glance.
private struct MachinesBadge: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        let online = app.machines.filter(\.connected).count
        HStack(spacing: 4) {
            Image(systemName: "laptopcomputer.and.iphone")
            if !app.machines.isEmpty {
                Circle().fill(online == app.machines.count ? Color.green : online == 0 ? Color.red : Color.yellow).frame(width: 7, height: 7)
            }
        }
    }
}
