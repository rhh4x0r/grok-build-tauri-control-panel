import SwiftUI

/// A Codex thread's goal, above the composer: what it is, how it's going, and Pause / Resume /
/// Clear. Or, when Codex turned a `/goal` down, that it didn't start and why. Same card as the Mac's.
struct GoalCard: View {
    let goal: GoalView
    let thread: ThreadModel
    @State private var error: String?

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: symbol)
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(tint)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 2) {
                Text(error ?? goal.headline)
                    .font(Theme.caption)
                    .foregroundStyle(error != nil ? Theme.danger : goal.kind == .notStarted || goal.kind == .needsYou ? Theme.warning : Theme.textMuted)
                    .lineLimit(1)
                Text(goal.title)
                    .font(Theme.small)
                    .foregroundStyle(Theme.text)
                    .lineLimit(goal.kind == .notStarted ? 3 : 1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if goal.canPause {
                action("Pause", "pause.fill") { await run("pause") }
            }
            if goal.canResume {
                action("Resume", "play.fill", prominent: true) { await run("resume") }
                    .disabled(thread.turnActive)
            }
            if goal.canClear {
                iconButton(goal.kind == .done ? "Dismiss" : "Clear goal") { await run("clear") }
            } else {
                iconButton("Dismiss") { thread.dismissGoalNotice() }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(RoundedRectangle(cornerRadius: Theme.panelCorner).fill(Theme.glass))
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: Theme.panelCorner))
        .overlay(RoundedRectangle(cornerRadius: Theme.panelCorner).stroke(Theme.hairline, lineWidth: 1))
        .onChange(of: goal) { error = nil }
    }

    private var symbol: String {
        switch goal.kind {
        case .working: "point.topleft.down.to.point.bottomright.curvepath"
        case .paused: "pause.circle"
        case .needsYou, .notStarted: "exclamationmark.triangle"
        case .done: "checkmark.circle"
        }
    }

    private var tint: Color {
        switch goal.kind {
        case .working: Theme.accent
        case .paused, .done: Theme.textMuted
        case .needsYou, .notStarted: Theme.warning
        }
    }

    private func run(_ action: String) async {
        error = nil
        do { try await thread.goalControl(action) } catch { self.error = describe(error) }
    }

    private func action(_ title: String, _ systemImage: String, prominent: Bool = false, _ perform: @escaping () async -> Void) -> some View {
        Button { Task { await perform() } } label: {
            Label(title, systemImage: systemImage)
                .labelStyle(.titleAndIcon)
                .font(Theme.sans(13, .medium))
                .foregroundStyle(prominent ? Theme.onSolid : Theme.text)
                .padding(.horizontal, 10)
                .frame(minHeight: 30)
                .background(Capsule().fill(prominent ? Theme.solid : Color.clear))
                .overlay(Capsule().stroke(prominent ? Color.clear : Theme.pillBorder, lineWidth: 1))
        }
        .buttonStyle(.plain)
    }

    private func iconButton(_ label: String, _ perform: @escaping () async -> Void) -> some View {
        Button { Task { await perform() } } label: {
            Image(systemName: "xmark")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(Theme.textMuted)
                .frame(width: 28, height: 28)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}
