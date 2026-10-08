import SwiftUI
import VisionKit

/// Paired Macs and servers, and pairing new ones.
struct MachinesView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @State private var pairing = false
    var prefilled: String?

    var body: some View {
        NavigationStack {
            List {
                Section {
                    ForEach(app.machines) { machine in
                        HStack(spacing: 12) {
                            Image(systemName: machine.isMac ? "laptopcomputer" : "server.rack").frame(width: 24)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(machine.name)
                                Text(Theme.linkText(machine.link)).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                            }
                            Spacer()
                            Circle().fill(Theme.linkColor(machine.link)).frame(width: 8, height: 8)
                        }
                        .swipeActions {
                            Button("Remove", role: .destructive) { Task { await app.remove(machine) } }
                        }
                    }
                } footer: {
                    Text("Away from home, the phone reaches your Mac and servers through Tailscale.")
                }
                Section {
                    Button("Pair a Mac or server", systemImage: "qrcode.viewfinder") { pairing = true }
                }
            }
            .navigationTitle("Machines")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
            .sheet(isPresented: $pairing) { PairSheet(code: prefilled ?? "") }
            .onAppear { if prefilled != nil { pairing = true } }
        }
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

    var body: some View {
        NavigationStack {
            Form {
                if DataScannerViewController.isSupported && outcomes.isEmpty && code.isEmpty {
                    Section {
                        QRScanner { scanned in code = scanned }
                            .frame(height: 280)
                            .listRowInsets(EdgeInsets())
                    }
                }
                Section {
                    TextField("bomb://pair…", text: $code, axis: .vertical)
                        .lineLimit(1...4)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    if let hosts = try? pairingHosts(text: code), outcomes.isEmpty {
                        ForEach(hosts, id: \.self) { Label($0, systemImage: "network") }
                    }
                } header: {
                    Text("Pairing code")
                } footer: {
                    Text("On your Mac: Bomb Code → Settings → Phone. One code pairs the Mac and the servers it's linked to.")
                }
                ForEach(outcomes, id: \.host) { outcome in
                    Label(outcome.machine?.name ?? outcome.host, systemImage: outcome.machine != nil ? "checkmark.circle.fill" : "exclamationmark.triangle")
                        .foregroundStyle(outcome.machine != nil ? .green : .orange)
                    if let error = outcome.error { Text(error).font(.caption).foregroundStyle(.secondary) }
                }
                if let error { Text(error).foregroundStyle(.red) }
            }
            .navigationTitle("Pair")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button(outcomes.isEmpty ? "Cancel" : "Done") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    if outcomes.isEmpty {
                        Button("Pair") { Task { await pair() } }.disabled(working || (try? pairingHosts(text: code)) == nil)
                    }
                }
            }
        }
    }

    private func pair() async {
        working = true
        defer { working = false }
        do { outcomes = try await app.pair(code: code) } catch { self.error = describe(error) }
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
