import SwiftUI

/// Agent replies: paragraphs, headings, lists and code blocks, with inline markdown inside each.
struct MarkdownText: View {
    let source: String

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(Block.parse(source).enumerated()), id: \.offset) { _, block in
                switch block {
                case let .paragraph(text):
                    Text(inline(text))
                case let .heading(level, text):
                    Text(inline(text)).font(level == 1 ? .title3.bold() : level == 2 ? .headline : .subheadline.bold())
                case let .bullet(marker, text):
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(marker).foregroundStyle(.secondary)
                        Text(inline(text))
                    }
                case let .code(text):
                    ScrollView(.horizontal, showsIndicators: false) {
                        Text(text).font(.system(.footnote, design: .monospaced)).textSelection(.enabled).padding(10)
                    }
                    .background(Theme.bubble, in: RoundedRectangle(cornerRadius: 10))
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .textSelection(.enabled)
    }

    private func inline(_ text: String) -> AttributedString {
        (try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(text)
    }

    enum Block {
        case paragraph(String)
        case heading(Int, String)
        case bullet(String, String)
        case code(String)

        static func parse(_ source: String) -> [Block] {
            var blocks: [Block] = []
            var paragraph: [String] = []
            var code: [String]?
            func flush() {
                if !paragraph.isEmpty { blocks.append(.paragraph(paragraph.joined(separator: "\n"))) }
                paragraph = []
            }
            for line in source.components(separatedBy: "\n") {
                let trimmed = line.trimmingCharacters(in: .whitespaces)
                if trimmed.hasPrefix("```") {
                    if let open = code {
                        blocks.append(.code(open.joined(separator: "\n")))
                        code = nil
                    } else {
                        flush()
                        code = []
                    }
                    continue
                }
                if code != nil { code?.append(line); continue }
                if trimmed.isEmpty { flush(); continue }
                if let hashes = trimmed.firstIndex(where: { $0 != "#" }), trimmed.hasPrefix("#"), trimmed[hashes] == " " {
                    flush()
                    let level = trimmed.distance(from: trimmed.startIndex, to: hashes)
                    blocks.append(.heading(level, String(trimmed[hashes...]).trimmingCharacters(in: .whitespaces)))
                } else if trimmed.hasPrefix("- ") || trimmed.hasPrefix("* ") {
                    flush()
                    blocks.append(.bullet("•", String(trimmed.dropFirst(2))))
                } else if let dot = trimmed.firstIndex(of: "."), trimmed[..<dot].allSatisfy(\.isNumber), !trimmed[..<dot].isEmpty,
                          trimmed[trimmed.index(after: dot)...].hasPrefix(" ") {
                    flush()
                    blocks.append(.bullet(String(trimmed[...dot]), String(trimmed[trimmed.index(dot, offsetBy: 2)...])))
                } else {
                    paragraph.append(line)
                }
            }
            if let open = code { blocks.append(.code(open.joined(separator: "\n"))) }
            flush()
            return blocks
        }
    }
}
