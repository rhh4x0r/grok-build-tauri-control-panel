import SwiftUI

/// Follows the system appearance, like the desktop app: neutral panels, hairline borders, quiet chrome.
enum Theme {
    static let bubble = Color(uiColor: .secondarySystemBackground)
    static let panel = Color(uiColor: .secondarySystemGroupedBackground)
    static let hairline = Color(uiColor: .separator)
    static let muted = Color.secondary
    static let corner: CGFloat = 18

    static func statusColor(_ summary: ThreadSummary) -> Color {
        if summary.needsApproval { return .orange }
        if summary.running { return .green }
        switch summary.status {
        case "failed": return .red
        default: return Color(uiColor: .tertiaryLabel)
        }
    }

    static func linkColor(_ link: LinkState) -> Color {
        switch link {
        case .connected: .green
        case .connecting: .yellow
        case .offline: .red
        case .paused: Color(uiColor: .tertiaryLabel)
        }
    }

    static func linkText(_ link: LinkState) -> String {
        switch link {
        case .connected: "Connected"
        case .connecting: "Connecting…"
        case let .offline(reason): reason
        case .paused: "Paused"
        }
    }
}

/// "game" from "/home/max/projects/game".
func projectName(_ path: String) -> String {
    URL(fileURLWithPath: path).lastPathComponent
}

/// "5 min ago" from an RFC 3339 time.
func timeAgo(_ rfc3339: String) -> String {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    let date = formatter.date(from: rfc3339) ?? {
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: rfc3339)
    }()
    guard let date else { return "" }
    return RelativeDateTimeFormatter().localizedString(for: date, relativeTo: .now)
}

/// A message a person can read, from any error the core returns.
func describe(_ error: Error) -> String {
    if case let .Failed(message) = error as? MobileError { return message }
    return error.localizedDescription
}
