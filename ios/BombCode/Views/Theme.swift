import CoreText
import SwiftUI

/// Bomb Code's look, from the desktop app (`crates/bomb_app/src/theme.rs`): Geist and Geist Mono,
/// two designed appearances that follow the system (near-black over the ember artwork in dark,
/// clean white in light), neutral text tones, soft washes and hairlines, and colour only where it
/// means something (working, needs you, finished, failed, Claude's orange).
enum Theme {
    // MARK: Colour tokens (light, dark), as in `Ui::of`.

    static let bg = dynamic(light: .white, dark: grey(0x06))
    static let text = dynamic(light: neutral(0.25), dark: neutral(0.922))
    static let textMuted = dynamic(light: neutral(0.439), dark: neutral(0.708))
    static let textFaint = dynamic(light: neutral(0.535), dark: neutral(0.556))
    static let solid = dynamic(light: neutral(0.205), dark: neutral(0.922))
    static let onSolid = dynamic(light: neutral(0.985), dark: grey(0x0e))
    static let accent = dynamic(light: hex(0x4f46e5), dark: hex(0x818cf8))
    static let danger = dynamic(light: hex(0xdc2626), dark: hex(0xf87171))
    static let warning = dynamic(light: hex(0xb45309), dark: hex(0xfbbf24))
    static let success = dynamic(light: hex(0x059669), dark: hex(0x34d399))
    static let claude = dynamic(light: hex(0xb85c3a), dark: hex(0xd97757))
    /// User bubbles and code wells.
    static let bubble = dynamic(light: UIColor(white: 0, alpha: 0.06), dark: UIColor(white: 1, alpha: 0.08))
    /// Pressed rows and selected chips.
    static let wash = dynamic(light: UIColor(white: 0.10, alpha: 0.06), dark: UIColor(white: 0.92, alpha: 0.11))
    static let hairline = dynamic(light: UIColor(white: 0, alpha: 0.11), dark: UIColor(white: 1, alpha: 0.08))
    static let pillBorder = dynamic(light: UIColor(hue: 210 / 360, saturation: 0.18, brightness: 0.32, alpha: 0.14),
                                    dark: UIColor(hue: 210 / 360, saturation: 0.18, brightness: 0.78, alpha: 0.12))
    /// Cards and the composer: translucent over the artwork in dark, near-white in light.
    static let glass = dynamic(light: UIColor(white: 0.985, alpha: 1), dark: UIColor(white: 0.06, alpha: 0.72))
    /// The fuse: the burning progress bar while an agent works.
    static let fuse = LinearGradient(colors: [Color(hex: 0xf97316), Color(hex: 0xfbbf24)], startPoint: .leading, endPoint: .trailing)

    // MARK: Type, on the desktop scale (caption 12 · small 13 · body 14 · title 17 · display 24).

    static func sans(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        let name: String
        switch weight {
        case .bold, .heavy, .black: name = "Geist-Bold"
        case .semibold: name = "Geist-SemiBold"
        case .medium: name = "Geist-Medium"
        default: name = "Geist-Regular"
        }
        return .custom(name, size: size, relativeTo: .body)
    }

    static func mono(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        let name = weight == .bold || weight == .semibold ? "GeistMono-Bold" : weight == .medium ? "GeistMono-Medium" : "GeistMono-Regular"
        return .custom(name, size: size, relativeTo: .body)
    }

    static let caption = sans(12)
    static let small = sans(13)
    static let body = sans(14)
    /// Transcript prose: 15 on a 24 line, like the desktop.
    static let prose = sans(15)
    static let proseSpacing: CGFloat = 3
    static let title = sans(17, .semibold)

    static let corner: CGFloat = 16
    static let panelCorner: CGFloat = 12
    static let composerCorner: CGFloat = 26

    /// Bundled Geist fonts, registered once at launch.
    static func registerFonts() {
        for name in ["Geist", "Geist-Medium", "Geist-SemiBold", "Geist-Bold", "Geist-Italic", "GeistMono", "GeistMono-Medium", "GeistMono-Bold"] {
            if let url = Bundle.main.url(forResource: name, withExtension: "ttf") {
                CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
            }
        }
    }

    // MARK: Status

    static func linkColor(_ link: LinkState) -> Color {
        switch link {
        case .connected: success
        case .connecting: warning
        case .offline: danger
        case .paused: textFaint
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

    // MARK: Helpers

    private static func dynamic(light: UIColor, dark: UIColor) -> Color {
        Color(uiColor: UIColor { $0.userInterfaceStyle == .dark ? dark : light })
    }
    private static func grey(_ v: Int) -> UIColor { UIColor(white: CGFloat(v) / 255, alpha: 1) }
    private static func neutral(_ l: CGFloat) -> UIColor { UIColor(white: l, alpha: 1) }
    private static func hex(_ v: UInt32) -> UIColor {
        UIColor(red: CGFloat((v >> 16) & 0xff) / 255, green: CGFloat((v >> 8) & 0xff) / 255, blue: CGFloat(v & 0xff) / 255, alpha: 1)
    }
}

extension Color {
    init(hex v: UInt32) {
        self.init(red: Double((v >> 16) & 0xff) / 255, green: Double((v >> 8) & 0xff) / 255, blue: Double(v & 0xff) / 255)
    }
}

// MARK: - Shared pieces

/// The canvas behind every screen: the ember artwork in dark (as on the desktop), plain white in light.
struct BombBackground: View {
    /// How strongly the artwork shows: full on the list, dimmed behind reading.
    var strength: Double = 1
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        ZStack {
            Theme.bg
            if scheme == .dark {
                // Sized by the screen, never the other way round: a filling image would widen the layout.
                Color.clear
                    .overlay(alignment: .bottomTrailing) {
                        Image("Embers").resizable().scaledToFill().opacity(0.85 * strength)
                    }
                    .clipped()
                // Keep text legible over the brightest embers.
                LinearGradient(colors: [Theme.bg.opacity(0.9), Theme.bg.opacity(0.35), Theme.bg.opacity(0.6)], startPoint: .top, endPoint: .bottom)
            }
        }
        .ignoresSafeArea()
    }
}

/// The agent's mark: Claude in its orange, Codex and Grok in the text colour.
struct BrandMark: View {
    let backend: String
    var size: CGFloat = 14

    var body: some View {
        Group {
            switch backend {
            case "claude": Image("ClaudeMark").resizable().foregroundStyle(Theme.claude)
            case "codex": Image("OpenAIMark").resizable().foregroundStyle(Theme.text)
            case "grok": Image("GrokMark").resizable().foregroundStyle(Theme.text)
            default: Image(systemName: "sparkle").resizable().foregroundStyle(Theme.textMuted)
            }
        }
        .scaledToFit()
        .frame(width: size, height: size)
    }
}

/// The pixel bomb and the mono wordmark.
struct Wordmark: View {
    var size: CGFloat = 17

    var body: some View {
        HStack(spacing: 8) {
            Image("BombLogo").interpolation(.none).resizable().scaledToFit().frame(width: size + 9, height: size + 9)
            Text("Bomb Code").font(Theme.mono(size, .semibold)).foregroundStyle(Theme.text)
        }
    }
}

/// A rounded capsule with the desktop's pill border, for composer and header controls.
struct PillLabel<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        HStack(spacing: 6) { content }
            .font(Theme.small)
            .foregroundStyle(Theme.textMuted)
            .padding(.horizontal, 10)
            .frame(height: 30)
            .background(Capsule().fill(Theme.wash.opacity(0.5)))
            .overlay(Capsule().stroke(Theme.pillBorder, lineWidth: 1))
    }
}

/// A glass card with a hairline edge.
struct GlassCard<Content: View>: View {
    var stroke: Color = Theme.hairline
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) { content }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: Theme.panelCorner).fill(Theme.glass))
            .background(.ultraThinMaterial.opacity(0.4), in: RoundedRectangle(cornerRadius: Theme.panelCorner))
            .overlay(RoundedRectangle(cornerRadius: Theme.panelCorner).stroke(stroke, lineWidth: 1))
    }
}

/// Solid (the main action) or outline pill buttons, as in the desktop's cards.
struct BombButtonStyle: ButtonStyle {
    var prominent = false
    var tint: Color? = nil

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(Theme.sans(14, .medium))
            .foregroundStyle(prominent ? Theme.onSolid : (tint ?? Theme.text))
            .padding(.horizontal, 14)
            .frame(minHeight: 36)
            .frame(maxWidth: .infinity)
            .background(Capsule().fill(prominent ? (tint ?? Theme.solid) : Color.clear))
            .overlay(Capsule().stroke(prominent ? Color.clear : Theme.pillBorder, lineWidth: 1))
            .opacity(configuration.isPressed ? 0.7 : 1)
    }
}

/// Where a thread stands, in its row's corner: Working, Input or Failed in colour, else how long ago.
struct StatusCorner: View {
    let thread: ThreadSummary
    var unseen = false

    var body: some View {
        if thread.needsApproval {
            dot(Theme.warning, "Input")
        } else if thread.running {
            dot(Theme.accent, "Working")
        } else if thread.status == "failed" {
            dot(Theme.danger, "Failed")
        } else {
            HStack(spacing: 5) {
                if unseen { Circle().fill(Theme.success).frame(width: 6, height: 6) }
                Text(shortAgo(thread.updatedAt))
                    .font(Theme.sans(12, .medium))
                    .foregroundStyle(unseen ? Theme.text : Theme.textFaint)
            }
        }
    }

    private func dot(_ color: Color, _ word: String) -> some View {
        HStack(spacing: 5) {
            Circle().fill(color).frame(width: 6, height: 6)
            Text(word).font(Theme.sans(12, .medium)).foregroundStyle(color)
        }
    }
}

/// "game" from "/home/max/projects/game".
func projectName(_ path: String) -> String {
    URL(fileURLWithPath: path).lastPathComponent
}

private func parseDate(_ rfc3339: String) -> Date? {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    if let date = formatter.date(from: rfc3339) { return date }
    formatter.formatOptions = [.withInternetDateTime]
    return formatter.date(from: rfc3339)
}

/// "5 min ago" from an RFC 3339 time.
func timeAgo(_ rfc3339: String) -> String {
    guard let date = parseDate(rfc3339) else { return "" }
    return RelativeDateTimeFormatter().localizedString(for: date, relativeTo: .now)
}

/// "now", "49m", "3h", "2d", "4w", like the desktop sidebar.
func shortAgo(_ rfc3339: String) -> String {
    guard let date = parseDate(rfc3339) else { return "" }
    let s = max(0, Int(Date.now.timeIntervalSince(date)))
    switch s {
    case ..<60: return "now"
    case ..<3600: return "\(s / 60)m"
    case ..<86_400: return "\(s / 3600)h"
    case ..<(86_400 * 7): return "\(s / 86_400)d"
    case ..<(86_400 * 30): return "\(s / (86_400 * 7))w"
    default: return "\(s / (86_400 * 30))mo"
    }
}

/// A message a person can read, from any error the core returns.
func describe(_ error: Error) -> String {
    if case let .Failed(message) = error as? MobileError { return message }
    return error.localizedDescription
}
