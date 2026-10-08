import SwiftUI

/// Where a navigation link leads.
struct ThreadRef: Hashable {
    let machineId: String
    let threadId: String
}

/// Every thread on every paired machine, grouped by project, styled like the desktop sidebar.
struct ThreadList: View {
    @Environment(AppModel.self) private var app
    @Binding var path: [ThreadRef]
    @State private var search = ""
    @State private var showMachines = false
    @State private var showNew = false

    private var groups: [(key: String, root: String, machineId: String, threads: [ThreadSummary])] {
        let wanted = app.threads.filter { search.isEmpty || ($0.label ?? "").localizedCaseInsensitiveContains(search) || $0.projectRoot.localizedCaseInsensitiveContains(search) }
        var order: [String] = []
        var byProject: [String: [ThreadSummary]] = [:]
        for thread in wanted {
            let key = "\(thread.machineId)\u{0}\(thread.projectRoot)"
            if byProject[key] == nil { order.append(key) }
            byProject[key, default: []].append(thread)
        }
        return order.map { key in
            let threads = byProject[key] ?? []
            return (key, threads.first?.projectRoot ?? "", threads.first?.machineId ?? "", threads)
        }
    }

    var body: some View {
        ZStack {
            VStack(spacing: 0) {
                header
                if app.machines.isEmpty {
                    PairPrompt { showMachines = true }
                } else {
                    SearchField(text: $search).padding(.horizontal, 16).padding(.bottom, 8)
                    list
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background { BombBackground() }
        .toolbar(.hidden, for: .navigationBar)
        .sheet(isPresented: $showMachines) { MachinesView() }
        .sheet(isPresented: $showNew) {
            NewThreadSheet { ref in path.append(ref) }
        }
    }

    private var header: some View {
        HStack(spacing: 10) {
            Wordmark()
            Spacer()
            Button { showMachines = true } label: { MachinesPill() }
                .buttonStyle(.plain)
            Button { showNew = true } label: {
                Image(systemName: "square.and.pencil")
                    .font(.system(size: 15, weight: .medium))
                    .foregroundStyle(Theme.onSolid)
                    .frame(width: 34, height: 34)
                    .background(Circle().fill(Theme.solid))
            }
            .buttonStyle(.plain)
            .disabled(!app.machines.contains { $0.connected })
            .opacity(app.machines.contains { $0.connected } ? 1 : 0.35)
        }
        .padding(.horizontal, 16)
        .padding(.top, 8)
        .padding(.bottom, 12)
    }

    private var list: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 18) {
                ForEach(groups, id: \.key) { group in
                    VStack(alignment: .leading, spacing: 2) {
                        ProjectHeader(root: group.root, machine: app.machine(id: group.machineId))
                        ForEach(group.threads, id: \.id) { thread in
                            NavigationLink(value: ThreadRef(machineId: thread.machineId, threadId: thread.id)) {
                                ThreadRow(thread: thread)
                            }
                            .buttonStyle(RowPressStyle())
                            .contextMenu {
                                if thread.running {
                                    Button("Stop", systemImage: "stop.circle", role: .destructive) {
                                        Task { try? await app.machine(id: thread.machineId)?.machine.cancel(threadId: thread.id) }
                                    }
                                }
                            }
                        }
                    }
                }
                if app.threads.isEmpty {
                    EmptyThreads().padding(.top, 80)
                }
            }
            .padding(.horizontal, 10)
            .padding(.bottom, 24)
        }
        .scrollIndicators(.hidden)
        .refreshable {
            app.reconnectAll()
            for machine in app.machines { _ = try? await machine.machine.refreshThreads() }
        }
    }
}

/// A thread in the list: agent mark, title, and where it stands.
private struct ThreadRow: View {
    let thread: ThreadSummary

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            BrandMark(backend: thread.backend, size: 14)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 2) {
                Text(thread.label ?? "New thread")
                    .font(Theme.sans(15, thread.running || thread.needsApproval ? .medium : .regular))
                    .foregroundStyle(Theme.text)
                    .lineLimit(1)
                if !thread.model.isEmpty {
                    Text(thread.model)
                        .font(Theme.mono(11))
                        .foregroundStyle(Theme.textFaint)
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 8)
            StatusCorner(thread: thread)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 9)
        .contentShape(Rectangle())
    }
}

/// The desktop's row wash when a row is pressed.
private struct RowPressStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(RoundedRectangle(cornerRadius: 10).fill(configuration.isPressed ? Theme.wash : Color.clear))
    }
}

/// Project name, and the machine it lives on.
private struct ProjectHeader: View {
    let root: String
    let machine: MachineModel?

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "folder")
                .font(.system(size: 13, weight: .regular))
                .foregroundStyle(Theme.textFaint)
            Text(projectName(root))
                .font(Theme.sans(14, .medium))
                .foregroundStyle(Theme.textMuted)
                .lineLimit(1)
            Spacer(minLength: 6)
            if let machine {
                HStack(spacing: 4) {
                    Image(systemName: machine.isMac ? "laptopcomputer" : "server.rack").font(.system(size: 10))
                    Text(machine.name).font(Theme.mono(11)).lineLimit(1)
                }
                .foregroundStyle(Theme.textFaint)
                .padding(.horizontal, 7)
                .frame(height: 20)
                .overlay(Capsule().stroke(Theme.pillBorder, lineWidth: 1))
            }
        }
        .padding(.horizontal, 10)
        .padding(.bottom, 4)
    }
}

/// How many machines are connected, at a glance.
private struct MachinesPill: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        let online = app.machines.filter(\.connected).count
        let color = app.machines.isEmpty ? Theme.textFaint : online == app.machines.count ? Theme.success : online == 0 ? Theme.danger : Theme.warning
        PillLabel {
            Circle().fill(color).frame(width: 7, height: 7)
            Text(app.machines.isEmpty ? "Pair" : "\(online)/\(app.machines.count)")
                .font(Theme.mono(12, .medium))
                .foregroundStyle(Theme.text)
        }
    }
}

private struct SearchField: View {
    @Binding var text: String

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").font(.system(size: 13)).foregroundStyle(Theme.textFaint)
            TextField("", text: $text, prompt: Text("Search threads").foregroundStyle(Theme.textFaint))
                .font(Theme.body)
                .foregroundStyle(Theme.text)
                .autocorrectionDisabled()
            if !text.isEmpty {
                Button { text = "" } label: { Image(systemName: "xmark.circle.fill").foregroundStyle(Theme.textFaint) }
                    .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 12)
        .frame(height: 38)
        .background(RoundedRectangle(cornerRadius: 12).fill(Theme.bubble))
    }
}

private struct PairPrompt: View {
    let pair: () -> Void

    var body: some View {
        VStack(spacing: 16) {
            Spacer()
            Image("BombLogo").interpolation(.none).resizable().scaledToFit().frame(width: 72, height: 72)
            Text("Pair with your Mac").font(Theme.sans(22, .semibold)).foregroundStyle(Theme.text)
            Text("Open Bomb Code on your Mac, go to Settings → Phone, and scan the code it shows.")
                .font(Theme.body).foregroundStyle(Theme.textMuted).multilineTextAlignment(.center)
                .frame(maxWidth: 280)
            Button("Scan pairing code", action: pair)
                .buttonStyle(BombButtonStyle(prominent: true))
                .frame(width: 220)
                .padding(.top, 6)
            Spacer()
            Spacer()
        }
        .padding(24)
    }
}

private struct EmptyThreads: View {
    var body: some View {
        VStack(spacing: 10) {
            Image("BombLogo").interpolation(.none).resizable().scaledToFit().frame(width: 44, height: 44).opacity(0.8)
            Text("No threads yet").font(Theme.sans(17, .semibold)).foregroundStyle(Theme.text)
            Text("Start one with the pencil.").font(Theme.small).foregroundStyle(Theme.textMuted)
        }
        .frame(maxWidth: .infinity)
    }
}
