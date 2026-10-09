import SwiftUI
import WebKit

/// A web page on the machine's own loopback (a dev server a thread started), to open on the phone.
struct PreviewTarget: Identifiable, Hashable {
    let port: UInt16
    /// Path, query and fragment ("/", "/login?x=1").
    let path: String
    var id: String { "\(port)\(path)" }
    var address: String { "localhost:\(port)\(path == "/" ? "" : path)" }
}

/// Finding `localhost` addresses in replies, and telling them apart from other links.
enum LocalLinks {
    private static let pattern = try! NSRegularExpression(
        pattern: #"(?:https?://)?(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\]):(\d{2,5})(/[^\s`'"<>)\]]*)?"#,
        options: [.caseInsensitive])

    /// The page a link names, when it's on the machine's loopback.
    static func target(_ url: URL) -> PreviewTarget? {
        guard let host = url.host?.lowercased(), ["localhost", "127.0.0.1", "0.0.0.0", "::1"].contains(host),
              let port = url.port, (1...65535).contains(port) else { return nil }
        var path = url.path.isEmpty ? "/" : url.path
        if let query = url.query { path += "?\(query)" }
        if let fragment = url.fragment { path += "#\(fragment)" }
        return PreviewTarget(port: UInt16(port), path: path)
    }

    /// Make every `localhost:PORT/…` in `text` a link (markdown leaves bare ones as plain text).
    static func mark(_ text: inout AttributedString) {
        let plain = String(text.characters)
        let matches = pattern.matches(in: plain, range: NSRange(plain.startIndex..., in: plain))
        var searchFrom = text.startIndex
        for match in matches {
            guard let range = Range(match.range, in: plain) else { continue }
            var found = String(plain[range])
            // A sentence's full stop or comma isn't part of the address.
            while let last = found.last, ".,;:!?".contains(last) { found.removeLast() }
            let address = found.lowercased().hasPrefix("http") ? found : "http://\(found)"
            guard let url = URL(string: address), target(url) != nil,
                  let at = text[searchFrom...].range(of: found) else { continue }
            if text[at].link == nil { text[at].link = url }
            searchFrom = at.upperBound
        }
    }
}

/// The page, loaded through this iPhone's port that carries to the machine (see `Machine.openPreview`).
struct PreviewBrowser: View {
    let machine: MachineModel
    let target: PreviewTarget
    @Environment(\.dismiss) private var dismiss
    @State private var page = WebPage()
    @State private var failed: String?

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Button { page.view.goBack() } label: { Image(systemName: "chevron.left") }
                    .disabled(!page.canGoBack)
                Button { page.view.reload() } label: { Image(systemName: "arrow.clockwise") }
                VStack(spacing: 1) {
                    Text(page.address ?? target.address).font(Theme.mono(12)).foregroundStyle(Theme.text).lineLimit(1)
                    Text("on \(machine.name)").font(Theme.caption).foregroundStyle(Theme.textFaint)
                }
                .frame(maxWidth: .infinity)
                Button("Done") { dismiss() }.font(Theme.sans(15, .medium))
            }
            .foregroundStyle(Theme.text)
            .padding(.horizontal, 16)
            .frame(height: 48)
            .background(Theme.bg)
            ZStack {
                WebViewHost(page: page)
                if let failed {
                    VStack(spacing: 8) {
                        Image(systemName: "network.slash").font(.system(size: 26)).foregroundStyle(Theme.textFaint)
                        Text("Couldn't open \(target.address)").font(Theme.sans(15, .semibold)).foregroundStyle(Theme.text)
                        Text(failed).font(Theme.small).foregroundStyle(Theme.textMuted).multilineTextAlignment(.center)
                    }
                    .padding(24)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(Theme.bg)
                }
            }
        }
        .task {
            do {
                let local = try await machine.machine.openPreview(port: target.port)
                page.port = (machine: target.port, phone: local)
                if let url = URL(string: "http://localhost:\(local)\(target.path)") { page.view.load(URLRequest(url: url)) }
            } catch {
                failed = describe(error)
                return
            }
            // The page's backend (an API on another port) has to answer here too: open every server
            // the Mac has running on its own port number, and any started while the page is open.
            while !Task.isCancelled {
                for server in (try? await machine.machine.localServers()) ?? [] where server.port != target.port {
                    _ = try? await machine.machine.openPreview(port: server.port)
                }
                try? await Task.sleep(for: .seconds(5))
            }
        }
        .onChange(of: page.error) { _, error in if let error { failed = error } }
        // Tunnels exist only while a page is open.
        .onDisappear { machine.machine.closePreviews() }
    }
}

/// The web view and what the bar shows about it.
@MainActor @Observable
final class WebPage: NSObject, WKNavigationDelegate {
    let view: WKWebView
    private(set) var canGoBack = false
    private(set) var address: String?
    private(set) var error: String?
    /// The machine's port and the phone's, to show the machine's in the address.
    var port: (machine: UInt16, phone: UInt16)?

    override init() {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        view = WKWebView(frame: .zero, configuration: configuration)
        super.init()
        view.navigationDelegate = self
        view.allowsBackForwardNavigationGestures = true
        view.isInspectable = true
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { update() }
    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) { error = nil; update() }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        let code = (error as NSError).code
        guard code != NSURLErrorCancelled else { return }
        self.error = code == NSURLErrorCannotConnectToHost || code == NSURLErrorNetworkConnectionLost
            ? "Nothing answered on that port on the Mac. Is the server still running?"
            : error.localizedDescription
    }

    private func update() {
        canGoBack = view.canGoBack
        guard let url = view.url, let port else { return }
        let shown = url.port.map(UInt16.init) == port.phone ? port.machine : url.port.map(UInt16.init) ?? port.machine
        let rest = url.path + (url.query.map { "?\($0)" } ?? "")
        address = "localhost:\(shown)\(rest == "/" ? "" : rest)"
    }
}

private struct WebViewHost: UIViewRepresentable {
    let page: WebPage
    func makeUIView(context: Context) -> WKWebView { page.view }
    func updateUIView(_ view: WKWebView, context: Context) {}
}

/// The servers a thread has running, for the bar's globe button.
struct ServersMenu: View {
    let servers: [ThreadServer]
    let open: (PreviewTarget) -> Void

    var body: some View {
        Menu {
            Section("Open on this iPhone") {
                ForEach(servers, id: \.port) { server in
                    Button("localhost:\(server.port) · \(server.name)", systemImage: "globe") {
                        open(PreviewTarget(port: server.port, path: "/"))
                    }
                }
            }
        } label: {
            Image(systemName: "globe")
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(Theme.text)
                .frame(width: 32, height: 32)
                .background(Circle().fill(Theme.bubble))
        }
        .accessibilityLabel("Open a running server")
    }
}
