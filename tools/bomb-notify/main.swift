// Bomb Code Notifier: posts the Mac app's notifications. macOS only lets an app bundle post them,
// and Bomb Code itself runs as a plain program, so crates/bomb_app/build.rs wraps this in
// "Bomb Code Notifier.app" next to the bomb_app binary. bomb_app runs it and talks over stdio:
//
//   in  (one JSON object per line):
//     {"cmd":"status"}                      how notifications are allowed, without asking
//     {"cmd":"authorize"}                   ask (macOS shows its prompt the first time only)
//     {"cmd":"notify","id":…,"title":…,"body":…,"thread":…}
//     {"cmd":"withdraw","thread":…}         take a thread's notifications away (it was opened)
//   out:
//     {"status":"notDetermined|denied|authorized|provisional"}
//     {"clicked":"<thread id>"}             a notification was clicked
//
// Started any other way (its notification clicked after Bomb Code quit), it opens nothing and exits.

import AppKit
import Foundation
import UserNotifications

func emit(_ object: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: object), let line = String(data: data, encoding: .utf8) else { return }
    print(line)
    fflush(stdout)
}

func statusName(_ status: UNAuthorizationStatus) -> String {
    switch status {
    case .authorized: "authorized"
    case .denied: "denied"
    case .provisional: "provisional"
    case .notDetermined: "notDetermined"
    default: "authorized"
    }
}

final class Notifier: NSObject, UNUserNotificationCenterDelegate {
    let center = UNUserNotificationCenter.current()

    func reportStatus() {
        center.getNotificationSettings { emit(["status": statusName($0.authorizationStatus)]) }
    }

    func authorize() {
        center.requestAuthorization(options: [.alert, .sound]) { _, _ in self.reportStatus() }
    }

    func notify(id: String, title: String, body: String, thread: String) {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.sound = .default
        content.threadIdentifier = thread
        content.userInfo = ["thread": thread]
        center.add(UNNotificationRequest(identifier: id, content: content, trigger: nil))
    }

    func withdraw(thread: String) {
        center.getDeliveredNotifications { delivered in
            let ids = delivered.filter { ($0.request.content.userInfo["thread"] as? String) == thread }.map(\.request.identifier)
            self.center.removeDeliveredNotifications(withIdentifiers: ids)
        }
    }

    // Shown even while this helper is "active" (it never is, but macOS asks).
    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void) {
        done([.banner, .sound, .list])
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                withCompletionHandler done: @escaping () -> Void) {
        if let thread = response.notification.request.content.userInfo["thread"] as? String { emit(["clicked": thread]) }
        done()
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.prohibited)
let notifier = Notifier()
notifier.center.delegate = notifier

// Only Bomb Code drives it (stdin is its pipe); clicked after Bomb Code quit, there's no one to tell.
if isatty(0) != 0 || CommandLine.arguments.contains("--check") {
    if CommandLine.arguments.contains("--check") { print("ok") }
    exit(0)
}

Thread.detachNewThread {
    while let line = readLine() {
        guard let data = line.data(using: .utf8), let command = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { continue }
        let text = { (key: String) in command[key] as? String ?? "" }
        DispatchQueue.main.async {
            switch text("cmd") {
            case "status": notifier.reportStatus()
            case "authorize": notifier.authorize()
            case "notify": notifier.notify(id: text("id"), title: text("title"), body: text("body"), thread: text("thread"))
            case "withdraw": notifier.withdraw(thread: text("thread"))
            default: break
            }
        }
    }
    exit(0)
}

app.run()
