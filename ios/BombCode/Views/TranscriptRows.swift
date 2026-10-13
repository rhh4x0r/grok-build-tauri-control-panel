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
        case let .question(entry): QuestionCard(entry: entry, thread: thread)
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
                    CopyIcon(text: entry.text)
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
                Label(planApproval ? planOutcome(resolution, options) : outcome(resolution, options), systemImage: resolution.hasPrefix("reject") || resolution == "cancelled" ? "xmark.circle" : "checkmark.circle")
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
                    if planApproval {
                        planActions(requestId)
                    } else {
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
                    }
                    if let allowPattern { Text("Always allowing adds \(allowPattern)").font(Theme.caption).foregroundStyle(Theme.textFaint) }
                    if let note { Text(note).font(Theme.caption).foregroundStyle(Theme.textFaint) }
                }
            }
        }
    }

    @State private var feedback = ""

    /// Implement the plan (choosing how edits are approved), or keep planning, with notes.
    @ViewBuilder private func planActions(_ requestId: String) -> some View {
        VStack(spacing: 8) {
            planButton(requestId, "auto", "Implement · auto-accept edits", prominent: true)
            planButton(requestId, "ask", "Implement · ask before edits", prominent: false)
            HStack(spacing: 8) {
                TextField("Or tell it what to change…", text: $feedback, axis: .vertical)
                    .font(Theme.sans(14)).lineLimit(1...4)
                    .padding(10).background(Theme.bubble, in: RoundedRectangle(cornerRadius: 8))
                Button {
                    Task { await plan(requestId, "keep") }
                } label: {
                    if sending == "keep" { ProgressView().controlSize(.small) } else { Text("Keep planning") }
                }
                .buttonStyle(BombButtonStyle(prominent: false, tint: nil))
                .disabled(sending != nil)
            }
        }
    }

    private func planButton(_ requestId: String, _ choice: String, _ label: String, prominent: Bool) -> some View {
        Button {
            Task { await plan(requestId, choice) }
        } label: {
            HStack(spacing: 8) {
                Text(label).lineLimit(1)
                if sending == choice { ProgressView().controlSize(.small) }
            }
        }
        .buttonStyle(BombButtonStyle(prominent: prominent, tint: nil))
        .disabled(sending != nil)
    }

    private func plan(_ requestId: String, _ choice: String) async {
        sending = choice
        defer { sending = nil }
        let notes = feedback.trimmingCharacters(in: .whitespacesAndNewlines)
        do {
            if try await !thread.answerPlan(requestId: requestId, choice: choice, feedback: choice == "keep" && !notes.isEmpty ? notes : nil) {
                note = "Already answered on another device."
            } else {
                feedback = ""
            }
        } catch {
            note = describe(error)
        }
    }

    private func planOutcome(_ resolution: String, _ options: [ApprovalChoice]) -> String {
        if resolution == "cancelled" { return "Plan cancelled" }
        if resolution == "restored" { return "Asked before this was reopened" }
        let kind = options.first { $0.id == resolution }?.kind ?? resolution
        return kind.hasPrefix("reject") ? "Kept planning" : "Plan approved"
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

/// The agent asks something (Codex's clarifying questions): pick a choice per question, or type.
struct QuestionCard: View {
    let entry: EntryView
    let thread: ThreadModel
    @State private var picks: [String: Set<String>] = [:]
    @State private var typed: [String: String] = [:]
    @State private var sending = false
    @State private var note: String?

    var body: some View {
        if case let .question(requestId, _, questions, answers, resolution) = entry.body {
            if let resolution {
                VStack(alignment: .leading, spacing: 4) {
                    Label(resolution == "declined" ? "Question skipped" : "Question answered", systemImage: resolution == "declined" ? "xmark.circle" : "checkmark.circle")
                        .font(Theme.sans(13, .medium)).foregroundStyle(Theme.textFaint)
                    ForEach(questions, id: \.id) { q in
                        let given = answers.filter { $0.questionId == q.id }.map(\.answer)
                        if !given.isEmpty {
                            Text("\(q.title): \(q.secret ? "••••" : given.joined(separator: ", "))").font(Theme.caption).foregroundStyle(Theme.textFaint)
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            } else {
                GlassCard(stroke: Theme.accent.opacity(0.55)) {
                    HStack(spacing: 6) {
                        Circle().fill(Theme.accent).frame(width: 6, height: 6)
                        Text("Question").font(Theme.sans(14, .semibold)).foregroundStyle(Theme.text)
                    }
                    ForEach(questions, id: \.id) { q in
                        VStack(alignment: .leading, spacing: 8) {
                            Text(q.title).font(Theme.sans(14, .medium)).foregroundStyle(Theme.text)
                            if let d = q.description, d != q.title { Text(d).font(Theme.small).foregroundStyle(Theme.textMuted) }
                            ForEach(q.choices, id: \.value) { choice in
                                Button {
                                    // One choice, or several for a multi-select; tapping again un-picks.
                                    var chosen = picks[q.id] ?? []
                                    if chosen.contains(choice.value) { chosen.remove(choice.value) }
                                    else if q.multi { chosen.insert(choice.value) }
                                    else { chosen = [choice.value] }
                                    picks[q.id] = chosen
                                } label: {
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(choice.label).lineLimit(2)
                                        if let d = choice.description { Text(d).font(Theme.caption).lineLimit(2) }
                                    }
                                }
                                .buttonStyle(BombButtonStyle(prominent: picks[q.id]?.contains(choice.value) == true, tint: nil))
                            }
                            if q.allowsTyping {
                                let field = Binding(get: { typed[q.id] ?? "" }, set: { typed[q.id] = $0 })
                                Group {
                                    if q.secret { SecureField(q.choices.isEmpty ? "Your answer" : "Or your own answer", text: field) }
                                    else { TextField(q.choices.isEmpty ? "Your answer" : "Or your own answer", text: field, axis: .vertical).lineLimit(1...4) }
                                }
                                .font(Theme.sans(14)).padding(10).background(Theme.bubble, in: RoundedRectangle(cornerRadius: 8))
                            }
                        }
                    }
                    HStack(spacing: 8) {
                        Button {
                            Task { await send(requestId, questions) }
                        } label: {
                            if sending { ProgressView().controlSize(.small) } else { Text("Send answers") }
                        }
                        .buttonStyle(BombButtonStyle(prominent: true, tint: nil))
                        .disabled(sending || !complete(questions))
                        Button("Skip") { Task { await skip(requestId) } }
                            .buttonStyle(BombButtonStyle(prominent: false, tint: nil))
                            .disabled(sending)
                    }
                    if let note { Text(note).font(Theme.caption).foregroundStyle(Theme.textFaint) }
                }
            }
        }
    }

    /// What answers a question: its picks, plus any typed words (a note when something is picked).
    private func answers(for q: QuestionView) -> [String] {
        let text = (typed[q.id] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        return (picks[q.id].map { Array($0) } ?? []) + (text.isEmpty ? [] : [text])
    }

    /// Every required question answered; optional ones (a free "Other") never block.
    private func complete(_ questions: [QuestionView]) -> Bool {
        questions.filter(\.required).allSatisfy { !answers(for: $0).isEmpty } && questions.contains { !answers(for: $0).isEmpty }
    }

    private func send(_ requestId: String, _ questions: [QuestionView]) async {
        let all = questions.flatMap { q in answers(for: q).map { QuestionAnswer(questionId: q.id, answer: $0) } }
        await submit(requestId, all)
    }

    private func skip(_ requestId: String) async {
        await submit(requestId, [])
    }

    private func submit(_ requestId: String, _ answers: [QuestionAnswer]) async {
        sending = true
        defer { sending = false }
        do {
            if try await !thread.answerQuestion(requestId: requestId, answers: answers) { note = "Already answered on another device." }
        } catch {
            note = describe(error)
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
