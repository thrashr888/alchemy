// alchemy-fm: Apple Foundation Models sidecar (RFC-inference-providers §4).
//
// One-shot per invocation, stateless:
//   --probe             print {"type":"probe","available":bool,"detail":…} and exit
//   (default)           read one NDJSON request from stdin:
//                         {"messages":[{"role":"system"|"user"|"assistant","content":…}]}
//                       stream NDJSON events to stdout:
//                         {"type":"token","text":…}        (delta, not snapshot)
//                         {"type":"done","chars":N}
//                         {"type":"error","message":…}
//
// The base SystemLanguageModel API is macOS 26+; on older systems every mode
// reports unavailable rather than failing to launch. The on-device model has
// a small context window — callers keep Small-role prompts tight and fall
// through to their chat engine on any error here.

import Foundation
#if canImport(FoundationModels)
import FoundationModels
#endif

struct Turn: Decodable {
    let role: String
    let content: String
}

struct Request: Decodable {
    let messages: [Turn]
}

func emit(_ obj: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: obj),
        let line = String(data: data, encoding: .utf8)
    else { return }
    print(line)
    FileHandle.standardOutput.synchronizeFile()
    fflush(stdout)
}

func probe() {
    #if canImport(FoundationModels)
    if #available(macOS 26.0, *) {
        let model = SystemLanguageModel.default
        switch model.availability {
        case .available:
            emit(["type": "probe", "available": true, "detail": "on-device model ready"])
        case .unavailable(let reason):
            emit(["type": "probe", "available": false, "detail": "\(reason)"])
        @unknown default:
            emit(["type": "probe", "available": false, "detail": "unknown availability"])
        }
        return
    }
    #endif
    emit(["type": "probe", "available": false, "detail": "requires macOS 26 or newer"])
}

func respond() async {
    guard let line = readLine(strippingNewline: true), !line.isEmpty else {
        emit(["type": "error", "message": "no request on stdin"])
        return
    }
    guard let data = line.data(using: .utf8),
        let request = try? JSONDecoder().decode(Request.self, from: data)
    else {
        emit(["type": "error", "message": "malformed request JSON"])
        return
    }

    #if canImport(FoundationModels)
    if #available(macOS 26.0, *) {
        guard case .available = SystemLanguageModel.default.availability else {
            emit(["type": "error", "message": "on-device model unavailable"])
            return
        }
        // System turns become session instructions; the rest replays into
        // one prompt (v1 one-shot stance — Alchemy reconstructs context).
        let instructions = request.messages.filter { $0.role == "system" }
            .map(\.content).joined(separator: "\n\n")
        let convo = request.messages.filter { $0.role != "system" }
        var prompt = ""
        if convo.count == 1 {
            prompt = convo[0].content
        } else {
            for turn in convo {
                prompt += (turn.role == "assistant" ? "Assistant: " : "User: ")
                prompt += turn.content + "\n\n"
            }
            prompt += "Assistant:"
        }

        do {
            let session = LanguageModelSession(instructions: instructions)
            var previous = ""
            // streamResponse yields cumulative snapshots; emit only deltas so
            // the Rust side can forward tokens verbatim.
            let stream = session.streamResponse(to: prompt)
            for try await partial in stream {
                let snapshot = partial.content
                if snapshot.hasPrefix(previous) {
                    let delta = String(snapshot.dropFirst(previous.count))
                    if !delta.isEmpty {
                        emit(["type": "token", "text": delta])
                    }
                } else {
                    // Model revised earlier text (rare): resend the whole
                    // snapshot as a correction marker.
                    emit(["type": "token", "text": snapshot, "replace": true])
                }
                previous = snapshot
            }
            emit(["type": "done", "chars": previous.count])
        } catch {
            emit(["type": "error", "message": "\(error)"])
        }
        return
    }
    #endif
    emit(["type": "error", "message": "requires macOS 26 or newer"])
}

if CommandLine.arguments.contains("--probe") {
    probe()
} else {
    await respond()
}
