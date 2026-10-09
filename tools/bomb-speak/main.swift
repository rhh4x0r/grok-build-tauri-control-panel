// bomb-speak: the Mac's read-aloud helper. Built by crates/bomb_app/build.rs together with
// ios/BombCode/Speech/SpeechEngine.swift, and placed next to the bomb_app binary.
//
//   bomb-speak --check   prints "ok" when this Mac has voices to read with
//   bomb-speak           reads one JSON command per line on stdin:
//     {"cmd":"voices"}  {"cmd":"preview","voice":id}  {"cmd":"personal_voice"}
//     {"cmd":"play","key":k,"sentences":[...],"voice":id,"rate":1.0,"from":0}
//     {"cmd":"pause"} {"cmd":"resume"} {"cmd":"seek","to":s} {"cmd":"skip","by":s} {"cmd":"rate","rate":r} {"cmd":"stop"}
//   and writes one JSON object per line: {"status":{...}}, {"voices":[...],"needsBetterVoice":b},
//   {"personalVoice":b}. Exits when stdin closes.

import AVFoundation
import Foundation

func emit(_ object: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: object), let line = String(data: data, encoding: .utf8) else { return }
    print(line)
    fflush(stdout)
}

func voicesMessage() -> [String: Any] {
    let scan = SpeechVoices.scan()
    let voices = scan.voices.map { v in
        ["id": v.id, "name": v.name, "language": v.language, "quality": v.quality, "personal": v.personal] as [String: Any]
    }
    return ["voices": voices, "needsBetterVoice": scan.needsBetter, "best": scan.best ?? ""]
}

if CommandLine.arguments.contains("--check") {
    print(AVSpeechSynthesisVoice.speechVoices().isEmpty ? "No voices are installed." : "ok")
    exit(0)
}

let player = SpeechPlayer()
player.onChange = { s in
    emit(["status": [
        "key": s.key, "playing": s.playing, "position": s.position, "duration": s.duration,
        "complete": s.complete, "sentence": s.sentence, "rendered": s.rendered, "total": s.total, "failed": s.failed as Any,
    ]])
}

// Newly downloaded voices show up without a restart (macOS 14 and later).
if #available(macOS 14.0, *) {
    NotificationCenter.default.addObserver(forName: AVSpeechSynthesizer.availableVoicesDidChangeNotification, object: nil, queue: .main) { _ in
        emit(voicesMessage())
    }
}

Thread.detachNewThread {
    while let line = readLine() {
        guard let data = line.data(using: .utf8), let command = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { continue }
        let number = { (key: String) in (command[key] as? NSNumber)?.doubleValue ?? 0 }
        DispatchQueue.main.async {
            switch command["cmd"] as? String {
            case "voices": emit(voicesMessage())
            case "preview": if let id = command["voice"] as? String { player.preview(voiceId: id) }
            case "personal_voice": SpeechVoices.requestPersonalVoice { granted in emit(["personalVoice": granted]); emit(voicesMessage()) }
            case "play":
                let sentences = command["sentences"] as? [String] ?? []
                let voice = command["voice"] as? String
                let key = command["key"] as? String ?? "message"
                player.play(key: key, sentences: sentences, voiceId: voice, rate: Float(number("rate") == 0 ? 1 : number("rate")), from: number("from"))
            case "pause": player.pause()
            case "resume": player.resume()
            case "seek": player.seek(to: number("to"))
            case "skip": player.skip(by: number("by"))
            case "rate": player.setRate(Float(number("rate")))
            case "stop": player.stop()
            default: break
            }
        }
    }
    exit(0)
}

dispatchMain()
