import SwiftUI
import VisionKit

/// Paired Macs and servers, and pairing new ones.
struct MachinesView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @State private var pairing = false
    var prefilled: String?

    var body: some View {
        VStack(spacing: 0) {
            SheetHeader(title: "Machines") { EmptyView() } trailing: {
                Button("Done") { dismiss() }.foregroundStyle(Theme.text)
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 10) {
                    if !app.machines.isEmpty {
                        SectionLabel("Paired")
                        VStack(spacing: 0) {
                            ForEach(Array(app.machines.enumerated()), id: \.element.id) { index, machine in
                                if index > 0 { Rectangle().fill(Theme.hairline).frame(height: 1).padding(.leading, 50) }
                                MachineRow(machine: machine)
                                    .contextMenu {
                                        Button("Remove", systemImage: "trash", role: .destructive) { Task { await app.remove(machine) } }
                                    }
                            }
                        }
                        .background(RoundedRectangle(cornerRadius: Theme.panelCorner).fill(Theme.glass))
                        .overlay(RoundedRectangle(cornerRadius: Theme.panelCorner).stroke(Theme.hairline, lineWidth: 1))
                        Footnote("Away from home, the phone reaches your Mac and servers through Tailscale. Press and hold a machine to remove it.")
                    }
                    Button { pairing = true } label: {
                        Label("Pair a Mac or server", systemImage: "qrcode.viewfinder")
                    }
                    .buttonStyle(BombButtonStyle(prominent: true))
                    .padding(.top, 14)
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 24)
            }
            .scrollIndicators(.hidden)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background { BombBackground(strength: 0.5) }
        .presentationDragIndicator(.visible)
        .sheet(isPresented: $pairing) { PairSheet(code: prefilled ?? "") }
        .onAppear { if prefilled != nil { pairing = true } }
    }
}

/// A paired machine: what it is, its name, and whether it's reachable.
private struct MachineRow: View {
    let machine: MachineModel

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: machine.isMac ? "laptopcomputer" : "server.rack")
                .font(.system(size: 15))
                .foregroundStyle(Theme.textMuted)
                .frame(width: 26)
            VStack(alignment: .leading, spacing: 3) {
                Text(machine.name).font(Theme.sans(15, .medium)).foregroundStyle(Theme.text).lineLimit(1)
                Text(Theme.linkText(machine.link)).font(Theme.mono(11)).foregroundStyle(Theme.textFaint).lineLimit(2)
            }
            Spacer(minLength: 8)
            Circle().fill(Theme.linkColor(machine.link)).frame(width: 7, height: 7)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 12)
        .contentShape(Rectangle())
    }
}

/// Scan the code from Settings → Phone on the Mac, or paste a link.
struct PairSheet: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @State var code: String
    @State private var outcomes: [PairOutcome] = []
    @State private var working = false
    @State private var error: String?

    private var hosts: [String]? { try? pairingHosts(text: code) }

    var body: some View {
        VStack(spacing: 0) {
            SheetHeader(title: "Pair") {
                Button(outcomes.isEmpty ? "Cancel" : "Done") { dismiss() }
            } trailing: { EmptyView() }
            ScrollView {
                VStack(alignment: .leading, spacing: 10) {
                    if DataScannerViewController.isSupported && outcomes.isEmpty && code.isEmpty {
                        QRScanner { scanned in code = scanned }
                            .frame(height: 300)
                            .clipShape(RoundedRectangle(cornerRadius: Theme.corner))
                            .overlay(RoundedRectangle(cornerRadius: Theme.corner).stroke(Theme.hairline, lineWidth: 1))
                            .padding(.bottom, 8)
                    }
                    if outcomes.isEmpty {
                        SectionLabel("Pairing code")
                        TextField("", text: $code, prompt: Text("bomb://pair…").foregroundStyle(Theme.textFaint), axis: .vertical)
                            .font(Theme.mono(13))
                            .foregroundStyle(Theme.text)
                            .lineLimit(1...4)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .padding(12)
                            .background(RoundedRectangle(cornerRadius: Theme.panelCorner).fill(Theme.bubble))
                        if let hosts {
                            HostChips(items: hosts)
                        }
                        Footnote("On your Mac: Bomb Code → Settings → Phone. One code pairs the Mac and the servers it's linked to.")
                        Button {
                            Task { await pair() }
                        } label: {
                            if working { ProgressView().tint(Theme.onSolid) } else { Text("Pair") }
                        }
                        .buttonStyle(BombButtonStyle(prominent: true))
                        .disabled(working || hosts == nil)
                        .opacity(hosts == nil ? 0.4 : 1)
                        .padding(.top, 14)
                    } else {
                        SectionLabel("Paired")
                        GlassCard {
                            ForEach(outcomes, id: \.host) { outcome in
                                OutcomeRow(outcome: outcome)
                            }
                        }
                    }
                    if let error {
                        Text(error).font(Theme.small).foregroundStyle(Theme.danger).padding(.horizontal, 4)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 24)
            }
            .scrollIndicators(.hidden)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background { BombBackground(strength: 0.5) }
        .presentationDragIndicator(.visible)
    }

    private func pair() async {
        working = true
        defer { working = false }
        error = nil
        do { outcomes = try await app.pair(code: code) } catch { self.error = describe(error) }
    }
}

/// How one machine in a pairing code went.
private struct OutcomeRow: View {
    let outcome: PairOutcome

    var body: some View {
        let ok = outcome.machine != nil
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: ok ? "checkmark.circle.fill" : "exclamationmark.triangle.fill")
                .font(.system(size: 14))
                .foregroundStyle(ok ? Theme.success : Theme.warning)
            VStack(alignment: .leading, spacing: 2) {
                Text(outcome.machine?.name ?? outcome.host).font(Theme.sans(15, .medium)).foregroundStyle(Theme.text)
                if let error = outcome.error {
                    Text(error).font(Theme.caption).foregroundStyle(Theme.textMuted)
                } else {
                    Text(outcome.host).font(Theme.mono(11)).foregroundStyle(Theme.textFaint)
                }
            }
        }
    }
}

/// The addresses in a pairing code, as mono chips.
private struct HostChips: View {
    let items: [String]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(items, id: \.self) { item in
                HStack(spacing: 5) {
                    Image(systemName: "network").font(.system(size: 10))
                    Text(item).font(Theme.mono(11)).lineLimit(1)
                }
                .foregroundStyle(Theme.textMuted)
                .padding(.horizontal, 8)
                .frame(height: 22)
                .overlay(Capsule().stroke(Theme.pillBorder, lineWidth: 1))
            }
        }
        .padding(.horizontal, 2)
    }
}

/// The camera, looking for a QR code.
private struct QRScanner: UIViewControllerRepresentable {
    let found: (String) -> Void

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let scanner = DataScannerViewController(recognizedDataTypes: [.barcode(symbologies: [.qr])], isHighlightingEnabled: true)
        scanner.delegate = context.coordinator
        try? scanner.startScanning()
        return scanner
    }

    func updateUIViewController(_ controller: DataScannerViewController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(found: found) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        let found: (String) -> Void
        init(found: @escaping (String) -> Void) { self.found = found }

        func dataScanner(_ scanner: DataScannerViewController, didAdd items: [RecognizedItem], allItems: [RecognizedItem]) {
            for case let .barcode(code) in items {
                if let text = code.payloadStringValue, text.hasPrefix("bomb://") {
                    scanner.stopScanning()
                    found(text)
                    return
                }
            }
        }
    }
}
