import SwiftUI

struct TranscriptRowView: View {
    let row: TranscriptRow
    let thread: ThreadModel

    var body: some View {
        switch row {
        case let .user(entry): UserBubble(entry: entry)
        case let .agent(entry): AgentReply(entry: entry, thread: thread)
        case let .activity(_, items): ActivityLine(items: items)
        case let .plan(entry): PlanCard(entry: entry)
        case let .approval(entry): ApprovalCard(entry: entry, thread: thread)
        case let .line(entry): SystemLine(entry: entry)
        case let .subagent(entry): SubagentCard(entry: entry, thread: thread)
        }
    }
}

struct UserBubble: View {
    let entry: EntryView

    var body: some View {
        HStack {
            Spacer(minLength: 48)
            VStack(alignment: .trailing, spacing: 6) {
                ForEach(Array(entry.images.enumerated()), id: \.offset) { _, image in
                    if let data = Data(base64Encoded: image.data), let ui = UIImage(data: data) {
                        Image(uiImage: ui).resizable().scaledToFit().frame(maxHeight: 180).clipShape(RoundedRectangle(cornerRadius: 12))
                    }
                }
                if !entry.text.isEmpty {
                    Text(entry.text)
                        .font(Theme.prose)
                        .lineSpacing(Theme.proseSpacing)
                        .foregroundStyle(Theme.text)
                        .textSelection(.enabled)
                        .padding(.horizontal, 14).padding(.vertical, 10)
                        .background(Theme.bubble, in: RoundedRectangle(cornerRadius: Theme.corner))
                }
            }
        }
    }
}

/// "Thought · Ran 4 commands", or what is running now; tap to see each step.
struct ActivityLine: View {
    let items: [EntryView]
    @State private var expanded = false

    private var steps: [ToolStep] {
        items.compactMap { if case let .tool(_, name, _, args, _) = $0.body { ToolStep(name: name, args: args) } else { nil } }
    }

    private var label: String {
        if let running = items.last(where: { if case let .tool(_, _, status, _, _) = $0.body { status == "running" || status == "pending" } else { false } }),
           case let .tool(_, name, _, args, _) = running.body {
            return runningLabel(step: ToolStep(name: name, args: args))
        }
        let thoughts = items.filter { $0.role == .thought }.count
        return activityLabel(thoughts: UInt32(thoughts), steps: steps)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button { withAnimation(.snappy) { expanded.toggle() } } label: {
                HStack(spacing: 6) {
                    Text(label).lineLimit(1)
                    Image(systemName: "chevron.right").font(.system(size: 9, weight: .semibold)).rotationEffect(.degrees(expanded ? 90 : 0))
                }
                .font(Theme.sans(13, .medium))
                .foregroundStyle(Theme.textFaint)
            }
            .buttonStyle(.plain)
            if expanded {
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(items, id: \.id) { item in StepDetail(entry: item) }
                }
                .padding(.leading, 12)
                .overlay(alignment: .leading) { Rectangle().fill(Theme.hairline).frame(width: 1) }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct StepDetail: View {
    let entry: EntryView

    var body: some View {
        switch entry.body {
        case let .tool(_, name, status, args, result):
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Text(name).font(Theme.sans(13, .medium)).foregroundStyle(Theme.text)
                    Text(status).font(Theme.mono(11)).foregroundStyle(status == "failed" ? Theme.danger : Theme.textFaint)
                }
                if !args.isEmpty {
                    Text(args.prefix(600)).font(Theme.mono(12)).foregroundStyle(Theme.textMuted).lineLimit(8)
                }
                if let result, !result.isEmpty {
                    Text(result.prefix(1200)).font(Theme.mono(12)).foregroundStyle(Theme.textMuted).lineLimit(12)
                        .padding(8).frame(maxWidth: .infinity, alignment: .leading)
                        .background(Theme.bubble, in: RoundedRectangle(cornerRadius: 8))
                }
            }
        case let .text(text):
            Text(text).font(Theme.sans(13)).foregroundStyle(Theme.textMuted).italic()
        default:
            EmptyView()
        }
    }
}

struct PlanCard: View {
    let entry: EntryView

    var body: some View {
        if case let .plan(title, markdown) = entry.body {
            GlassCard {
                HStack(spacing: 6) {
                    Image(systemName: "list.bullet.clipboard").font(.system(size: 12, weight: .semibold))
                    Text(title ?? "Plan").font(Theme.sans(14, .semibold))
                }
                .foregroundStyle(Theme.text)
                MarkdownText(source: markdown)
            }
        }
    }
}

/// A question from the agent. Open cards show their choices; answered ones shrink to one line.
struct ApprovalCard: View {
    let entry: EntryView
    let thread: ThreadModel
    @State private var sending: String?
    @State private var note: String?

    var body: some View {
        if case let .approval(requestId, tool, summary, explanation, options, planApproval, allowPattern, resolution) = entry.body {
            if let resolution {
                Label(outcome(resolution, options), systemImage: resolution.hasPrefix("reject") || resolution == "cancelled" ? "xmark.circle" : "checkmark.circle")
                    .font(Theme.sans(13, .medium)).foregroundStyle(Theme.textFaint)
                    .frame(maxWidth: .infinity, alignment: .leading)
            } else {
                GlassCard(stroke: Theme.warning.opacity(0.55)) {
                    HStack(spacing: 6) {
                        Circle().fill(Theme.warning).frame(width: 6, height: 6)
                        Text(planApproval ? "Ready to build this plan?" : "\(tool) wants to run")
                            .font(Theme.sans(14, .semibold)).foregroundStyle(Theme.text)
                    }
                    Text(summary).font(Theme.mono(12)).foregroundStyle(Theme.textMuted).lineLimit(10)
                        .padding(10).frame(maxWidth: .infinity, alignment: .leading)
                        .background(Theme.bubble, in: RoundedRectangle(cornerRadius: 8))
                    if let explanation { Text(explanation).font(Theme.small).foregroundStyle(Theme.textMuted) }
                    VStack(spacing: 8) {
                        ForEach(Array(options.enumerated()), id: \.element.id) { index, option in
                            Button {
                                Task { await answer(requestId, option.id) }
                            } label: {
                                HStack(spacing: 8) {
                                    Text(option.label).lineLimit(1)
                                    if sending == option.id { ProgressView().controlSize(.small) }
                                }
                            }
                            // The first allow is the main action; denying stays visible but quiet.
                            .buttonStyle(BombButtonStyle(prominent: index == 0 && !option.kind.hasPrefix("reject"),
                                                         tint: option.kind.hasPrefix("reject") ? Theme.danger : nil))
                            .disabled(sending != nil)
                        }
                    }
                    if let allowPattern { Text("Always allowing adds \(allowPattern)").font(Theme.caption).foregroundStyle(Theme.textFaint) }
                    if let note { Text(note).font(Theme.caption).foregroundStyle(Theme.textFaint) }
                }
            }
        }
    }

    private func answer(_ requestId: String, _ optionId: String) async {
        sending = optionId
        defer { sending = nil }
        do {
            if try await !thread.answer(requestId: requestId, optionId: optionId) { note = "Already answered on another device." }
        } catch {
            note = describe(error)
        }
    }

    private func outcome(_ resolution: String, _ options: [ApprovalChoice]) -> String {
        switch resolution {
        case "cancelled": return "Cancelled"
        case "restored": return "Asked before this was reopened"
        default:
            let kind = options.first { $0.id == resolution }?.kind ?? resolution
            if kind.hasPrefix("reject") { return "Denied" }
            return kind == "allow_always" ? "Always allowed" : "Allowed"
        }
    }
}

struct SystemLine: View {
    let entry: EntryView

    var body: some View {
        Text(entry.text)
            .font(Theme.sans(12))
            .foregroundStyle(entry.role == .error ? Theme.danger : Theme.textFaint)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// A subagent the thread started: what it's doing, and a way into its own transcript.
struct SubagentCard: View {
    let entry: EntryView
    let thread: ThreadModel

    var body: some View {
        if case let .tool(childId, name, status, task, _) = entry.body {
            let title = String(name.dropFirst(subagentPrefix.count))
            NavigationLink(value: ThreadRef(machineId: thread.machine.id, threadId: childId, subagent: title)) {
                HStack(spacing: 10) {
                    SubagentStatusIcon(status: status)
                    VStack(alignment: .leading, spacing: 2) {
                        HStack(spacing: 6) {
                            Text(title).font(Theme.sans(14, .medium)).foregroundStyle(Theme.text)
                            Text("Subagent").font(Theme.mono(10, .medium)).foregroundStyle(Theme.textFaint)
                        }
                        if !task.isEmpty {
                            Text(task).font(Theme.caption).foregroundStyle(Theme.textMuted).lineLimit(2)
                        }
                    }
                    Spacer(minLength: 4)
                    Image(systemName: "chevron.right").font(.system(size: 11, weight: .semibold)).foregroundStyle(Theme.textFaint)
                }
                .padding(12)
                .background(RoundedRectangle(cornerRadius: Theme.panelCorner).fill(Theme.glass))
                .overlay(RoundedRectangle(cornerRadius: Theme.panelCorner).stroke(Theme.hairline, lineWidth: 1))
            }
            .buttonStyle(.plain)
        }
    }
}

/// Working, finished, or failed.
struct SubagentStatusIcon: View {
    let status: String

    var body: some View {
        switch status {
        case "running", "pending":
            ProgressView().controlSize(.small).tint(Theme.accent).frame(width: 18)
        case "completed":
            Image(systemName: "checkmark.circle.fill").foregroundStyle(Theme.success).frame(width: 18)
        default:
            Image(systemName: "xmark.circle.fill").foregroundStyle(Theme.danger).frame(width: 18)
        }
    }
}
