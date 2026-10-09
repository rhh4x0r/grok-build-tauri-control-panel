// bomb-dictate: the Mac's dictation helper. Built by crates/bomb_app/build.rs together with
// ios/BombCode/Speech/Dictation.swift, and placed next to the bomb_app binary.
//
//   bomb-dictate --check   prints "ok", or why dictation can't run here
//   bomb-dictate           records; prints one JSON object per line: {"level": 0…1} as sound comes
//                          in, then once stdin closes (or gets a line) and the words settle,
//                          {"text": "..."} and {"done": true}. {"error": "..."} on failure.

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
            try await dictation.start { level in emit(["level": Double(level)]) }
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
            let text = await dictation.stop()
            emit(["text": text])
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
