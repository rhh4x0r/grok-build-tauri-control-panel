import SwiftUI

@main
struct BombCodeApp: App {
    @State private var app = AppModel()
    @Environment(\.scenePhase) private var phase

    init() {
        Theme.registerFonts()
    }

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
                        VStack(spacing: 10) {
                            Image(systemName: "laptopcomputer.slash").font(.system(size: 30)).foregroundStyle(Theme.textFaint)
                            Text("That machine isn’t paired anymore").font(Theme.sans(17, .semibold)).foregroundStyle(Theme.text)
                        }
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .background { BombBackground(strength: 0.5) }
                    }
                }
        }
        .tint(Theme.text)
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
