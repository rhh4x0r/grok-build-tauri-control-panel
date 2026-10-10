import SwiftUI
import UIKit

@main
struct BombCodeApp: App {
    @State private var app = AppModel()
    @Environment(\.scenePhase) private var phase

    init() {
        Theme.registerFonts()
        _ = PhoneNotifications.shared
    }

    /// The background time asked for when the app leaves the screen.
    @State private var grace: UIBackgroundTaskIdentifier = .invalid

    private func beginGrace() {
        guard grace == .invalid else { return }
        grace = UIApplication.shared.beginBackgroundTask(withName: "Hear threads finish") {
            // Out of time: pause before iOS suspends the app.
            app.setActive(false)
            endGrace()
        }
        if grace == .invalid { app.setActive(false) }
    }

    private func endGrace() {
        guard grace != .invalid else { return }
        UIApplication.shared.endBackgroundTask(grace)
        grace = .invalid
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(app)
        }
        // Background App Refresh: catch up on threads that finished while the app was suspended.
        .backgroundTask(.appRefresh(PhoneNotifications.refreshTask)) {
            await PhoneNotifications.shared.backgroundCheck(app)
        }
        .onChange(of: phase) { _, phase in
            switch phase {
            case .active:
                endGrace()
                app.setActive(true)
                PhoneNotifications.shared.sceneChanged(active: true)
            case .background:
                PhoneNotifications.shared.sceneChanged(active: false)
                // Stay connected for the extra time iOS gives an app that's finishing something
                // (about 30 seconds), so a thread that finishes just after you switch away still
                // notifies; then pause cleanly (iOS would close the sockets anyway).
                beginGrace()
            default:
                break
            }
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
                        ThreadScreen(machine: machine, threadId: ref.threadId, subagent: ref.subagent)
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
        // A tapped notification opens its thread.
        .onChange(of: PhoneNotifications.shared.route) { _, route in
            guard let route else { return }
            path = [route]
            PhoneNotifications.shared.route = nil
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
