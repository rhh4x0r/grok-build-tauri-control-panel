// bomb-dictate: the Mac's dictation helper. Built by crates/bomb_app/build.rs together with
// ios/BombCode/Speech/Dictation.swift, and placed next to the bomb_app binary.
//
//   bomb-dictate --check   prints "ok", or why dictation can't run here
//   bomb-dictate           listens; prints one JSON object per line as words arrive:
//                          {"final": "...", "volatile": "..."}, then {"done": true} once stdin
//                          closes (or gets a line) and the last words settle. {"error": "..."} on failure.

import Foundation

func emit(_ object: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: object), let line = String(data: data, encoding: .utf8) else { return }
    print(line)
    fflush(stdout)
}

@available(macOS 26.0, *)
func run() -> Never {
    if CommandLine.arguments.contains("--check") {
        print(Dictation.isAvailable ? "ok" : "Dictation isn't available on this Mac.")
        exit(0)
    }
    let dictation = Dictation()
    Task { @MainActor in
        do {
            try await dictation.start { final, volatile in emit(["final": final, "volatile": volatile]) }
            emit(["listening": true])
        } catch {
            emit(["error": error.localizedDescription])
            exit(1)
        }
    }
    // Stop when the app closes stdin or writes a line.
    Thread.detachNewThread {
        _ = readLine()
        Task { @MainActor in
            await dictation.stop()
            emit(["done": true])
            exit(0)
        }
    }
    dispatchMain()
}

if #available(macOS 26.0, *) {
    run()
} else if CommandLine.arguments.contains("--check") {
    print("Dictation needs macOS 26 or later.")
} else {
    emit(["error": "Dictation needs macOS 26 or later."])
}
