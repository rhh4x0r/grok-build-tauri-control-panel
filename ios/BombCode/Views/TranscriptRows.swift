import SwiftUI

struct TranscriptRowView: View {
    let row: TranscriptRow
    let thread: ThreadModel

    var body: some View {
        switch row {
        case let .user(entry): UserBubble(entry: entry)
        case let .agent(entry): MarkdownText(source: entry.text).padding(.horizontal, 2)
        case let .activity(_, items): ActivityLine(items: items)
        case let .plan(entry): PlanCard(entry: entry)
        case let .approval(entry): ApprovalCard(entry: entry, thread: thread)
        case let .line(entry): SystemLine(entry: entry)
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
                    Image(systemName: "chevron.right").font(.caption2).rotationEffect(.degrees(expanded ? 90 : 0))
                }
                .font(.subheadline)
                .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            if expanded {
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(items, id: \.id) { item in StepDetail(entry: item) }
                }
                .padding(.leading, 10)
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
                    Text(name).font(.footnote.weight(.medium))
                    Text(status).font(.caption).foregroundStyle(status == "failed" ? .red : .secondary)
                }
                if !args.isEmpty {
                    Text(args.prefix(600)).font(.system(.caption, design: .monospaced)).foregroundStyle(.secondary).lineLimit(8)
                }
                if let result, !result.isEmpty {
                    Text(result.prefix(1200)).font(.system(.caption, design: .monospaced)).lineLimit(12)
                }
            }
        case let .text(text):
            Text(text).font(.footnote).foregroundStyle(.secondary).italic()
        default:
            EmptyView()
        }
    }
}

struct PlanCard: View {
    let entry: EntryView

    var body: some View {
        if case let .plan(title, markdown) = entry.body {
            VStack(alignment: .leading, spacing: 8) {
                Label(title ?? "Plan", systemImage: "list.bullet.clipboard").font(.subheadline.weight(.semibold))
                MarkdownText(source: markdown)
            }
            .padding(14)
            .background(Theme.panel, in: RoundedRectangle(cornerRadius: 14))
            .overlay(RoundedRectangle(cornerRadius: 14).stroke(Theme.hairline, lineWidth: 0.5))
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
                    .font(.subheadline).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
            } else {
                VStack(alignment: .leading, spacing: 10) {
                    Label(planApproval ? "Ready to build this plan?" : "\(tool) wants to run", systemImage: "hand.raised")
                        .font(.subheadline.weight(.semibold))
                    Text(summary).font(.system(.footnote, design: .monospaced)).lineLimit(10)
                    if let explanation { Text(explanation).font(.footnote).foregroundStyle(.secondary) }
                    ForEach(options, id: \.id) { option in
                        Button {
                            Task { await answer(requestId, option.id) }
                        } label: {
                            HStack {
                                Text(option.label)
                                Spacer()
                                if sending == option.id { ProgressView() }
                            }
                            .frame(maxWidth: .infinity)
                        }
                        .buttonStyle(.bordered)
                        .tint(option.kind.hasPrefix("reject") ? .red : option.kind == "allow_once" ? .accentColor : .primary)
                        .disabled(sending != nil)
                    }
                    if let allowPattern { Text("Always allowing adds \(allowPattern)").font(.caption).foregroundStyle(.secondary) }
                    if let note { Text(note).font(.caption).foregroundStyle(.secondary) }
                }
                .padding(14)
                .background(Theme.panel, in: RoundedRectangle(cornerRadius: 14))
                .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color.orange.opacity(0.6), lineWidth: 1))
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
            .font(.footnote)
            .foregroundStyle(entry.role == .error ? .red : .secondary)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}
