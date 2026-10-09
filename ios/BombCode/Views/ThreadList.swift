import SwiftUI

/// Where a navigation link leads.
struct ThreadRef: Hashable {
    let machineId: String
    let threadId: String
    /// Set for a subagent's transcript: its name. Subagents are read-only.
    var subagent: String? = nil
}

/// How the list orders projects and threads, as in the desktop sidebar.
enum ListSort: String, CaseIterable {
    case recent, created, alphabetical
    var label: String {
        switch self {
        case .recent: "Most recent"
        case .created: "Time created"
        case .alphabetical: "Alphabetical"
        }
    }
}

/// How far back a project's threads show before "Show more", like the desktop sidebar's setting.
enum RecentWindow: String, CaseIterable {
    case off, day = "1d", threeDays = "3d", week = "1w", month = "1m"
    var label: String {
        switch self {
        case .off: "Off"
        case .day: "1 day"
        case .threeDays: "3 days"
        case .week: "1 week"
        case .month: "1 month"
        }
    }
    var seconds: TimeInterval? {
        switch self {
        case .off: nil
        case .day: 86_400
        case .threeDays: 3 * 86_400
        case .week: 7 * 86_400
        case .month: 30 * 86_400
        }
    }
    /// The oldest time that still counts, as an RFC 3339 UTC prefix: thread times are UTC RFC 3339, so
    /// text order is time order.
    func cutoff(now: Date = .now) -> String? {
        guard let seconds else { return nil }
        let format = DateFormatter()
        format.locale = Locale(identifier: "en_US_POSIX")
        format.timeZone = TimeZone(identifier: "UTC")
        format.dateFormat = "yyyy-MM-dd'T'HH:mm:ss"
        return format.string(from: now.addingTimeInterval(-seconds))
    }
}

/// Which threads the list shows.
enum ListShow: String, CaseIterable {
    case all, working, input
    var label: String {
        switch self {
        case .all: "All threads"
        case .working: "Working"
        case .input: "Needs you"
        }
    }
    var icon: String {
        switch self {
        case .all: "tray.full"
        case .working: "bolt"
        case .input: "hand.raised"
        }
    }
}

/// One project on one machine, with the threads the list shows for it.
struct ProjectGroup {
    let key: String
    let root: String
    let machineId: String
    let pinned: Bool
    let threads: [ThreadSummary]
}

/// Every thread on every paired machine, grouped by project, organised like the desktop sidebar.
struct ThreadList: View {
    @Environment(AppModel.self) private var app
    @Binding var path: [ThreadRef]
    @State private var search = ""
    @State private var showMachines = false
    @State private var showNew = false
    @State private var showVoice = false
    /// Test mode: a preview opened straight away (BOMB_SMOKE_PREVIEW=port).
    @State private var smokePreview: PreviewTarget?
    @State private var expanded: Set<String> = []
    @AppStorage("list.sort") private var sort: ListSort = .recent
    @AppStorage("list.show") private var show: ListShow = .all
    @AppStorage("list.archived") private var showArchived = false
    @AppStorage("list.collapsed") private var collapsedRaw = ""
    @AppStorage("list.recent") private var recent: RecentWindow = .threeDays

    /// Recent rows per project before "Show more", like the sidebar.
    private let keep = 3

    private var collapsed: Set<String> { Set(collapsedRaw.split(separator: "\n").map(String.init)) }

    private func toggleCollapsed(_ key: String) {
        var set = collapsed
        if set.remove(key) == nil { set.insert(key) }
        collapsedRaw = set.sorted().joined(separator: "\n")
    }

    private var groups: [ProjectGroup] {
        var order: [String] = []
        var byProject: [String: [ThreadSummary]] = [:]
        for machine in app.machines {
            for thread in machine.threads {
                if !showArchived && machine.isArchived(thread) { continue }
                switch show {
                case .all: break
                case .working: if !thread.running || thread.needsApproval { continue }
                case .input: if !thread.needsApproval { continue }
                }
                if !search.isEmpty && !(thread.label ?? "").localizedCaseInsensitiveContains(search)
                    && !thread.projectRoot.localizedCaseInsensitiveContains(search) { continue }
                let key = "\(thread.machineId)\u{0}\(thread.projectRoot)"
                if byProject[key] == nil { order.append(key) }
                byProject[key, default: []].append(thread)
            }
        }
        let groups = order.map { key in
            let threads = sorted(byProject[key] ?? [])
            let first = threads.first
            let pinned = app.machine(id: first?.machineId ?? "")?.prefs.pinnedProjects.contains(first?.projectRoot ?? "") ?? false
            return ProjectGroup(key: key, root: first?.projectRoot ?? "", machineId: first?.machineId ?? "", pinned: pinned, threads: threads)
        }
        return groups.sorted { a, b in
            let name = projectName(a.root).localizedCaseInsensitiveCompare(projectName(b.root)) == .orderedAscending
            switch sort {
            case .recent:
                let (x, y) = (a.threads.map(\.updatedAt).max() ?? "", b.threads.map(\.updatedAt).max() ?? "")
                return x != y ? x > y : name
            case .created:
                let (x, y) = (a.threads.map(\.createdAt).max() ?? "", b.threads.map(\.createdAt).max() ?? "")
                return x != y ? x > y : name
            case .alphabetical:
                return name
            }
        }
    }

    private func sorted(_ threads: [ThreadSummary]) -> [ThreadSummary] {
        threads.sorted { a, b in
            let name = (a.label ?? "").localizedCaseInsensitiveCompare(b.label ?? "") == .orderedAscending
            switch sort {
            case .recent: return a.updatedAt != b.updatedAt ? a.updatedAt > b.updatedAt : name
            case .created: return a.createdAt != b.createdAt ? a.createdAt > b.createdAt : name
            case .alphabetical: return name
            }
        }
    }

    /// True when the list hides something by choice, so the button shows it.
    private var filtering: Bool { show != .all || showArchived || sort != .recent }

    var body: some View {
        VStack(spacing: 0) {
            header
            if app.machines.isEmpty {
                PairPrompt { showMachines = true }
            } else {
                HStack(spacing: 8) {
                    SearchField(text: $search)
                    organizeMenu
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 8)
                list
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background { BombBackground() }
        .toolbar(.hidden, for: .navigationBar)
        .sheet(isPresented: $showMachines) { MachinesView() }
        .sheet(isPresented: $showVoice) { VoiceSettings() }
        .fullScreenCover(item: $smokePreview) { target in
            if let machine = app.machines.first(where: \.connected) { PreviewBrowser(machine: machine, target: target) }
        }
        // Back at the list, the thread being read is closed.
        .onAppear { if ReadAloud.shared.playing != nil { ReadAloud.shared.close() } }
        .sheet(isPresented: $showNew) {
            NewThreadSheet { ref in path.append(ref) }
        }
        #if DEBUG
        .task {
            // Test mode can open a sheet for a screenshot: BOMB_SMOKE_SHEET=machines|new.
            guard Smoke.enabled, let sheet = ProcessInfo.processInfo.environment["BOMB_SMOKE_SHEET"] else { return }
            try? await Task.sleep(for: .seconds(4))
            if sheet == "machines" { showMachines = true } else if sheet == "new" { showNew = true } else if sheet == "voice" { showVoice = true }
            if sheet.hasPrefix("preview:"), let port = UInt16(sheet.dropFirst(8)) { smokePreview = PreviewTarget(port: port, path: "/") }
        }
        #endif
    }

    private var header: some View {
        HStack(spacing: 10) {
            Wordmark()
            Spacer()
            Button { showVoice = true } label: {
                Image(systemName: "gearshape")
                    .font(.system(size: 15, weight: .medium))
                    .foregroundStyle(Theme.textMuted)
                    .frame(width: 34, height: 34)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Voice settings")
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

    /// Sort, filter, and archived threads, in one menu beside search.
    private var organizeMenu: some View {
        Menu {
            Picker("Show", selection: $show) {
                ForEach(ListShow.allCases, id: \.self) { Label($0.label, systemImage: $0.icon).tag($0) }
            }
            .pickerStyle(.inline)
            Picker("Sort by", selection: $sort) {
                ForEach(ListSort.allCases, id: \.self) { Text($0.label).tag($0) }
            }
            .pickerStyle(.menu)
            Picker("Recent threads", selection: $recent) {
                ForEach(RecentWindow.allCases, id: \.self) { Text($0 == .off ? "Off · tap a project to see its threads" : "Last \($0.label)").tag($0) }
            }
            .pickerStyle(.menu)
            Toggle("Show archived", systemImage: "archivebox", isOn: $showArchived)
            if !collapsed.isEmpty {
                Button("Expand all projects", systemImage: "rectangle.expand.vertical") { collapsedRaw = "" }
            }
        } label: {
            Image(systemName: "line.3.horizontal.decrease")
                .font(.system(size: 14, weight: .medium))
                .foregroundStyle(filtering ? Theme.onSolid : Theme.textMuted)
                .frame(width: 38, height: 38)
                .background(RoundedRectangle(cornerRadius: 12).fill(filtering ? Theme.solid : Theme.bubble))
        }
    }

    private var list: some View {
        let groups = groups
        let pinned = groups.filter(\.pinned)
        let rest = groups.filter { !$0.pinned }
        return ScrollView {
            LazyVStack(alignment: .leading, spacing: 18) {
                if show != .all {
                    FilterChip(show: show) { show = .all }
                }
                if !pinned.isEmpty {
                    SectionLabel("Pinned").padding(.horizontal, 6).padding(.bottom, -8)
                    ForEach(pinned, id: \.key) { projectGroup($0) }
                    if !rest.isEmpty { SectionLabel("Projects").padding(.horizontal, 6).padding(.bottom, -8) }
                }
                ForEach(rest, id: \.key) { projectGroup($0) }
                if groups.isEmpty {
                    EmptyThreads(show: show, searching: !search.isEmpty, anyThreads: !app.threads.isEmpty).padding(.top, 80)
                }
            }
            .padding(.horizontal, 10)
            .padding(.bottom, 24)
        }
        .scrollIndicators(.hidden)
        .refreshable {
            app.reconnectAll()
            for machine in app.machines {
                _ = try? await machine.machine.refreshThreads()
                await machine.loadPrefs()
            }
        }
    }

    private func projectGroup(_ group: ProjectGroup) -> some View {
        let isCollapsed = collapsed.contains(group.key) && search.isEmpty
        // Searching or filtering shows every match; otherwise, like the desktop sidebar, the few
        // newest from the recent window, and anything working or waiting on you.
        let limited = search.isEmpty && show == .all && !expanded.contains(group.key)
        let cutoff = recent.cutoff()
        let visible = limited
            ? group.threads.enumerated().filter { index, thread in
                thread.running || (cutoff.map { index < keep && thread.updatedAt >= $0 } ?? false)
            }.map(\.element)
            : group.threads
        let hidden = group.threads.count - visible.count
        let machine = app.machine(id: group.machineId)
        return VStack(alignment: .leading, spacing: 2) {
            Button {
                withAnimation(.snappy(duration: 0.2)) { toggleCollapsed(group.key) }
            } label: {
                ProjectHeader(root: group.root, machine: machine, collapsed: isCollapsed,
                              busy: group.threads.contains { $0.running }, count: group.threads.count)
            }
            .buttonStyle(.plain)
            if !isCollapsed {
                ForEach(visible, id: \.id) { thread in
                    NavigationLink(value: ThreadRef(machineId: thread.machineId, threadId: thread.id)) {
                        ThreadRow(thread: thread)
                            .opacity(machine?.isArchived(thread) == true ? 0.55 : 1)
                    }
                    .buttonStyle(RowPressStyle())
                    .contextMenu {
                        if thread.running {
                            Button("Stop", systemImage: "stop.circle", role: .destructive) {
                                Task { try? await machine?.machine.cancel(threadId: thread.id) }
                            }
                        }
                    }
                }
                if hidden > 0 {
                    Button {
                        withAnimation(.snappy(duration: 0.2)) { _ = expanded.insert(group.key) }
                    } label: {
                        Text("Show \(hidden) more")
                            .font(Theme.sans(13, .medium))
                            .foregroundStyle(Theme.textFaint)
                            .padding(.horizontal, 38)
                            .padding(.vertical, 6)
                    }
                    .buttonStyle(.plain)
                } else if !limited && expanded.contains(group.key) && search.isEmpty && show == .all {
                    Button {
                        withAnimation(.snappy(duration: 0.2)) { _ = expanded.remove(group.key) }
                    } label: {
                        Text("Show less")
                            .font(Theme.sans(13, .medium))
                            .foregroundStyle(Theme.textFaint)
                            .padding(.horizontal, 38)
                            .padding(.vertical, 6)
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }
}

/// The active filter, with a way out.
private struct FilterChip: View {
    let show: ListShow
    let clear: () -> Void

    var body: some View {
        Button(action: clear) {
            HStack(spacing: 6) {
                Image(systemName: show.icon).font(.system(size: 11, weight: .medium))
                Text(show.label).font(Theme.sans(13, .medium))
                Image(systemName: "xmark").font(.system(size: 9, weight: .bold)).foregroundStyle(Theme.textFaint)
            }
            .foregroundStyle(Theme.text)
            .padding(.horizontal, 10)
            .frame(height: 28)
            .background(Capsule().fill(Theme.wash))
            .overlay(Capsule().stroke(Theme.pillBorder, lineWidth: 1))
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 6)
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

/// Project name and the machine it lives on; tap to fold the project away.
private struct ProjectHeader: View {
    let root: String
    let machine: MachineModel?
    let collapsed: Bool
    let busy: Bool
    let count: Int

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "folder")
                .font(.system(size: 13, weight: .regular))
                .foregroundStyle(Theme.textFaint)
            Text(projectName(root))
                .font(Theme.sans(14, .medium))
                .foregroundStyle(Theme.textMuted)
                .lineLimit(1)
            Image(systemName: "chevron.right")
                .font(.system(size: 9, weight: .semibold))
                .foregroundStyle(Theme.textFaint)
                .rotationEffect(.degrees(collapsed ? 0 : 90))
            if collapsed {
                if busy { Circle().fill(Theme.accent).frame(width: 6, height: 6) }
                Text("\(count)").font(Theme.mono(11)).foregroundStyle(Theme.textFaint)
            }
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
        .padding(.vertical, 4)
        .contentShape(Rectangle())
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
    let show: ListShow
    let searching: Bool
    let anyThreads: Bool

    private var title: String {
        if searching { return "No matches" }
        switch show {
        case .working: return "Nothing working"
        case .input: return "Nothing needs you"
        case .all: return "No threads yet"
        }
    }

    var body: some View {
        VStack(spacing: 10) {
            Image("BombLogo").interpolation(.none).resizable().scaledToFit().frame(width: 44, height: 44).opacity(0.8)
            Text(title).font(Theme.sans(17, .semibold)).foregroundStyle(Theme.text)
            if !anyThreads {
                Text("Start one with the pencil.").font(Theme.small).foregroundStyle(Theme.textMuted)
            }
        }
        .frame(maxWidth: .infinity)
    }
}
