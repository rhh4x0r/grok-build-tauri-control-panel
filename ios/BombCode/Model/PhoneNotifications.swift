import BackgroundTasks
import Foundation
import Observation
import UIKit
import UserNotifications

/// Telling you when a thread on your Mac is done: finished, waiting for your approval, or failed.
///
/// The phone notices it in the thread list the Mac sends while Bomb Code is open or just
/// backgrounded, and posts a local notification (not for the thread you're looking at). Once iOS
/// suspends the app, Background App Refresh wakes it now and then to catch up: iOS decides when,
/// so those can be late. A snapshot of every thread's state, kept between launches, is what a
/// wake compares against. No push service is involved.
@MainActor @Observable
final class PhoneNotifications {
    static let shared = PhoneNotifications()
    static let refreshTask = "com.mn.bombcode.ios.refresh"

    /// What iOS allows, once known.
    private(set) var status: UNAuthorizationStatus = .notDetermined
    private(set) var backgroundRefresh: UIBackgroundRefreshStatus = .available
    private(set) var lowPower = ProcessInfo.processInfo.isLowPowerModeEnabled
    /// The person chose to have them; the card was answered; "Not now" puts it off until then.
    private(set) var enabled = UserDefaults.standard.bool(forKey: "notify.enabled")
    private(set) var asked = UserDefaults.standard.bool(forKey: "notify.asked")
    private var snoozedUntil = UserDefaults.standard.double(forKey: "notify.snoozed")
    /// The card shows only once the app has been used for real (paired, or a message sent).
    private(set) var used = UserDefaults.standard.bool(forKey: "notify.used")
    /// The thread on screen, which gets no notifications.
    var openThread: String?
    /// A tapped notification's thread, for the navigation stack to open.
    var route: ThreadRef?

    private var active = true
    private let delegate = Delegate()

    private init() {
        UNUserNotificationCenter.current().delegate = delegate
        NotificationCenter.default.addObserver(forName: .NSProcessInfoPowerStateDidChange, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated { PhoneNotifications.shared.lowPower = ProcessInfo.processInfo.isLowPowerModeEnabled }
        }
    }

    // MARK: Asking

    var shouldOffer: Bool {
        used && !enabled && status != .denied && (!asked || (snoozedUntil > 0 && Date().timeIntervalSince1970 >= snoozedUntil))
    }

    /// Something real happened (paired, or a message sent): the card may show from now on.
    func noteUsed() {
        guard !used else { return }
        used = true
        UserDefaults.standard.set(true, forKey: "notify.used")
    }

    /// "Turn on": iOS's own prompt (shown once; after "Don't Allow" only Settings can change it).
    func turnOn() async {
        enabled = true
        asked = true
        snoozedUntil = 0
        persist()
        _ = try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge])
        await refreshStatus()
    }

    func notNow() {
        asked = true
        snoozedUntil = Date().addingTimeInterval(7 * 24 * 3600).timeIntervalSince1970
        persist()
    }

    func turnOff() {
        enabled = false
        asked = true
        snoozedUntil = 0
        persist()
    }

    func openSettings() {
        if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) }
    }

    private func persist() {
        let d = UserDefaults.standard
        d.set(enabled, forKey: "notify.enabled")
        d.set(asked, forKey: "notify.asked")
        d.set(snoozedUntil, forKey: "notify.snoozed")
    }

    /// What iOS allows now (on launch, and back from Settings).
    func refreshStatus() async {
        status = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
        backgroundRefresh = UIApplication.shared.backgroundRefreshStatus
        lowPower = ProcessInfo.processInfo.isLowPowerModeEnabled
    }

    /// Something keeps notifications from arriving while the app is closed (for the settings note).
    var backgroundLimit: String? {
        guard enabled, status == .authorized || status == .provisional else { return nil }
        if backgroundRefresh != .available { return "Background App Refresh is off for Bomb Code, so notifications only arrive while it's open." }
        if lowPower { return "Low Power Mode pauses background updates, so notifications only arrive while Bomb Code is open." }
        return nil
    }

    // MARK: Noticing

    private enum State: String { case running, approval, failed, idle }

    private static func state(_ thread: ThreadSummary) -> State {
        if thread.needsApproval { return .approval }
        if thread.running { return .running }
        return thread.status == "failed" ? .failed : .idle
    }

    private var snapshot: [String: String] = UserDefaults.standard.dictionary(forKey: "notify.snapshot") as? [String: String] ?? [:]

    /// A machine's latest thread list: notify about what changed since the last one (or the last
    /// one before the app was suspended), then remember this one.
    func compare(machine: MachineModel, threads: [ThreadSummary]) {
        for thread in threads {
            let key = "\(machine.id)/\(thread.id)"
            let now = Self.state(thread)
            let before = snapshot[key].flatMap(State.init)
            snapshot[key] = now.rawValue
            guard let before, before != now else { continue }
            switch (before, now) {
            case (.running, .idle), (.approval, .idle): post(machine: machine, thread: thread, body: "Done")
            case (.running, .failed), (.approval, .failed): post(machine: machine, thread: thread, body: "Failed")
            case (_, .approval): post(machine: machine, thread: thread, body: "Needs your approval")
            default: break
            }
        }
        UserDefaults.standard.set(snapshot, forKey: "notify.snapshot")
    }

    private func post(machine: MachineModel, thread: ThreadSummary, body: String) {
        guard enabled, status == .authorized || status == .provisional else { return }
        // Looking at it already.
        if active && openThread == thread.id { return }
        let content = UNMutableNotificationContent()
        content.title = thread.label ?? "Thread"
        content.subtitle = machine.name
        content.body = body
        content.sound = .default
        content.threadIdentifier = thread.id
        content.userInfo = ["machine": machine.id, "thread": thread.id]
        let request = UNNotificationRequest(identifier: "\(thread.id)-\(Date().timeIntervalSince1970)", content: content, trigger: nil)
        UNUserNotificationCenter.current().add(request)
    }

    /// A thread opened on screen: its notifications are no longer news.
    func opened(_ threadId: String) {
        openThread = threadId
        let center = UNUserNotificationCenter.current()
        center.getDeliveredNotifications { delivered in
            let ids = delivered.filter { $0.request.content.threadIdentifier == threadId }.map(\.request.identifier)
            center.removeDeliveredNotifications(withIdentifiers: ids)
        }
    }

    // MARK: In the background

    func sceneChanged(active: Bool) {
        self.active = active
        if active {
            Task { await refreshStatus() }
        } else {
            scheduleRefresh()
        }
    }

    /// Ask iOS to wake the app in a while to look again (iOS picks the time).
    func scheduleRefresh() {
        guard enabled else { return }
        let request = BGAppRefreshTaskRequest(identifier: Self.refreshTask)
        request.earliestBeginDate = Date(timeIntervalSinceNow: 15 * 60)
        try? BGTaskScheduler.shared.submit(request)
    }

    /// Woken in the background (about 30 seconds): reconnect, let the thread lists arrive (which
    /// posts what changed), then let go, and ask to be woken again.
    func backgroundCheck(_ app: AppModel) async {
        scheduleRefresh()
        guard enabled else { return }
        app.setActive(true)
        for _ in 0..<40 {
            if !app.machines.isEmpty && app.machines.allSatisfy({ $0.connected }) { break }
            try? await Task.sleep(for: .milliseconds(500))
        }
        for machine in app.machines where machine.connected {
            _ = try? await machine.machine.refreshThreads()
        }
        try? await Task.sleep(for: .seconds(2))
        app.setActive(false)
    }

    /// Notifications reach the app through this (iOS wants an NSObject).
    private final class Delegate: NSObject, UNUserNotificationCenterDelegate {
        func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
            [.banner, .list, .sound]
        }

        func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
            let info = response.notification.request.content.userInfo
            guard let machine = info["machine"] as? String, let thread = info["thread"] as? String else { return }
            await MainActor.run { PhoneNotifications.shared.route = ThreadRef(machineId: machine, threadId: thread) }
        }
    }
}
