import SwiftMath
import SwiftUI

/// A reply's text: markdown, with equations typeset between its paragraphs. Math inside a
/// sentence is already readable text (split in the shared core, the same as on the Mac).
struct ReplyText: View {
    let source: String

    var body: some View {
        let pieces = mathPieces(text: source)
        if case let .text(markdown)? = pieces.first, pieces.count == 1, markdown == source {
            MarkdownText(source: source)
        } else {
            VStack(alignment: .leading, spacing: 10) {
                ForEach(Array(pieces.enumerated()), id: \.offset) { _, piece in
                    switch piece {
                    case let .text(markdown):
                        // The text either side of an equation, without the spaces next to it.
                        let trimmed = markdown.trimmingCharacters(in: .whitespacesAndNewlines)
                        if !trimmed.isEmpty { MarkdownText(source: trimmed) }
                    case let .equation(tex):
                        Equation(tex: tex)
                    }
                }
            }
        }
    }
}

/// One equation, typeset natively by SwiftMath; wide ones scroll sideways. TeX it can't read is
/// shown as code.
struct Equation: View {
    let tex: String

    var body: some View {
        if MTMathListBuilder.build(fromString: tex) != nil {
            ScrollView(.horizontal, showsIndicators: false) {
                MathLabel(tex: tex)
                    .fixedSize()
                    .padding(.vertical, 4)
            }
            .frame(maxWidth: .infinity, alignment: .center)
        } else {
            Text(tex)
                .font(Theme.mono(13))
                .foregroundStyle(Theme.textMuted)
                .padding(8)
                .background(Theme.bubble, in: RoundedRectangle(cornerRadius: 8))
        }
    }
}

private struct MathLabel: UIViewRepresentable {
    let tex: String

    func makeUIView(context: Context) -> MTMathUILabel {
        let label = MTMathUILabel()
        label.labelMode = .display
        label.textAlignment = .center
        label.fontSize = 19
        return label
    }

    func updateUIView(_ label: MTMathUILabel, context: Context) {
        label.latex = tex
        label.textColor = UIColor(Theme.text)
        label.invalidateIntrinsicContentSize()
    }

    func sizeThatFits(_ proposal: ProposedViewSize, uiView label: MTMathUILabel, context: Context) -> CGSize? {
        label.intrinsicContentSize
    }
}
