// bomb-speak: the Mac's read-aloud helper. Built by crates/bomb_app/build.rs together with
// ios/BombCode/Speech/SpeechEngine.swift, and placed next to the bomb_app binary.
//
//   bomb-speak --check   prints "ok"
//   bomb-speak           reads one JSON command per line on stdin:
//     {"cmd":"voices","apiKey":k,"list":"recommended"|"popular"|"mine","query":q}
//     {"cmd":"preview","voice":id,"sample":url?,"apiKey":k}
//     {"cmd":"play","key":k,"sentences":[...],"voice":id,"apiKey":k,"model":m?,"rate":1.0,"from":0}
//     {"cmd":"check_key","apiKey":k}  {"cmd":"stop_preview"} {"cmd":"pause"} {"cmd":"resume"} {"cmd":"seek","to":s} {"cmd":"skip","by":s} {"cmd":"rate","rate":r} {"cmd":"stop"}
//   and writes one JSON object per line: {"status":{...}}, {"voices":[...]}, {"voicesError":"…"},
//   {"previewing":id|null}, {"keyCheck":{"ok":b,"error":"…"}}.
//   Exits when stdin closes. The API key is only ever sent to Fish Audio.

import AVFoundation
import Foundation

func emit(_ object: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: object), let line = String(data: data, encoding: .utf8) else { return }
    print(line)
    fflush(stdout)
}

if CommandLine.arguments.contains("--check") {
    print("ok")
    exit(0)
}

let player = SpeechPlayer()
player.onChange = { s in
    emit(["status": [
        "key": s.key, "playing": s.playing, "position": s.position, "duration": s.duration,
        "complete": s.complete, "sentence": s.sentence, "rendered": s.rendered, "total": s.total, "failed": s.failed as Any,
    ]])
}

player.onPreview = { voice in emit(["previewing": voice as Any]) }

/// The last voice search: a newer one replaces it.
var search: Task<Void, Never>?

Thread.detachNewThread {
    while let line = readLine() {
        guard let data = line.data(using: .utf8), let command = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { continue }
        let number = { (key: String) in (command[key] as? NSNumber)?.doubleValue ?? 0 }
        let text = { (key: String) in command[key] as? String }
        DispatchQueue.main.async {
            switch text("cmd") {
            case "voices":
                let (key, list, query) = (text("apiKey") ?? "", text("list") ?? "recommended", text("query") ?? "")
                search?.cancel()
                search = Task {
                    do {
                        let voices = try await FishAudio.voices(key: key, list: list, query: query)
                        guard !Task.isCancelled else { return }
                        emit(["voices": voices.map { v in
                            ["id": v.id, "name": v.name, "author": v.author, "languages": v.languages, "sample": v.sample as Any, "likes": v.likes] as [String: Any]
                        }, "list": list, "query": query])
                    } catch {
                        if !Task.isCancelled { emit(["voicesError": error.localizedDescription]) }
                    }
                }
            case "preview":
                if let id = text("voice") { player.preview(voiceId: id, sample: text("sample"), apiKey: text("apiKey") ?? "") }
            case "play":
                let sentences = command["sentences"] as? [String] ?? []
                player.play(key: text("key") ?? "message", sentences: sentences, voiceId: text("voice"), apiKey: text("apiKey") ?? "",
                            model: text("model") ?? FishAudio.defaultModel,
                            rate: Float(number("rate") == 0 ? 1 : number("rate")), from: number("from"))
            case "check_key":
                let key = text("apiKey") ?? ""
                Task {
                    do { try await FishAudio.validate(key: key); emit(["keyCheck": ["ok": true]]) }
                    catch { emit(["keyCheck": ["ok": false, "error": error.localizedDescription]]) }
                }
            case "stop_preview": player.stopPreview()
            case "pause": player.pause()
            case "resume": player.resume()
            case "seek": player.seek(to: number("to"))
            case "skip": player.skip(by: number("by"))
            case "rate": player.setRate(Float(number("rate")))
            case "stop": player.stop(); player.stopPreview()
            default: break
            }
        }
    }
    exit(0)
}

dispatchMain()
