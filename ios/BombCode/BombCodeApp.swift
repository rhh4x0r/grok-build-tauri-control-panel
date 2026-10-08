import SwiftUI

@main
struct BombCodeApp: App {
    @State private var app = AppModel()
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(app)
        }
        .onChange(of: phase) { _, phase in
            // iOS closes sockets in the background anyway; pause cleanly and reconnect on return.
            app.setActive(phase == .active)
        }
    }
}

struct RootView: View {
    @Environment(AppModel.self) private var app
    @State private var path: [ThreadRef] = []
    @State private var pairingLink: String?

    var body: some View {
        NavigationStack(path: $path) {
            ThreadList(path: $path)
                .navigationDestination(for: ThreadRef.self) { ref in
                    if let machine = app.machine(id: ref.machineId) {
                        ThreadScreen(machine: machine, threadId: ref.threadId)
                    } else {
                        ContentUnavailableView("That machine isn’t paired anymore", systemImage: "laptopcomputer.slash")
                    }
                }
        }
        #if DEBUG
        .task { await Smoke.run(app) { path.append($0) } }
        #endif
        // Opening a bomb://pair link (from the Camera app, or a message) goes straight to pairing.
        .onOpenURL { url in pairingLink = url.absoluteString }
        .sheet(item: Binding(get: { pairingLink.map(PairingLink.init) }, set: { pairingLink = $0?.id })) { link in
            PairSheet(code: link.id)
        }
    }
}

private struct PairingLink: Identifiable {
    let id: String
}
