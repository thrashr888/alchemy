// alchemy-fm: Apple Foundation Models sidecar (RFC-inference-providers §4).
//
// One-shot per invocation, stateless:
//   --probe             print {"type":"probe","available":bool,"detail":…} and exit
//   --share <folder>    show the macOS Share sheet on a folder; prints
//                       {"type":"presented"} once it is up and stays alive
//                       while the person uses it, or {"type":"error",…}
//                       (docs/RFC-shared-notebook.md §1)
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
#if canImport(AppKit)
import AppKit
#endif
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
    // fflush only: stdout is a pipe when the app drives us, and
    // synchronizeFile() (fsync) on a pipe raises NSFileHandleOperationException
    // → SIGABRT after the first token. fsync is for files; pipes need none.
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


// MARK: - The Share sheet (docs/RFC-shared-notebook.md §1)
//
// Sharing an iCloud Drive folder with another Apple ID is `NSSharingService`
// with the CloudSharing name — AppKit, which the Rust side has none of, and
// this sidecar already is. The caller treats every failure here as "show the
// folder in Finder and say which menu"; nothing below is load-bearing for
// the move itself, which has already happened by the time we run.

#if canImport(AppKit)
@available(macOS 10.12, *)
final class ShareDelegate: NSObject, NSSharingServiceDelegate, NSCloudSharingServiceDelegate {
    // The picker has no window to hang off in a process with no UI, so it is
    // anchored to the middle of the main screen.
    func sharingService(
        _ sharingService: NSSharingService, sourceFrameOnScreen forShareItem: Any
    ) -> NSRect {
        let frame = NSScreen.main?.frame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        return NSRect(x: frame.midX, y: frame.midY, width: 1, height: 1)
    }

    func sharingService(
        _ sharingService: NSSharingService, didCompleteForItems items: [Any], error: Error?
    ) {
        if let error { emit(["type": "error", "message": "\(error)"]) } else { emit(["type": "done"]) }
        exit(0)
    }

    func sharingService(
        _ sharingService: NSSharingService, didFailToShareItems items: [Any], error: Error
    ) {
        emit(["type": "error", "message": "\(error)"])
        exit(1)
    }
}
#endif

@MainActor
func share(_ path: String) {
    var directory: ObjCBool = false
    guard FileManager.default.fileExists(atPath: path, isDirectory: &directory), directory.boolValue
    else {
        emit(["type": "error", "message": "no folder at \(path)"])
        return
    }
    #if canImport(AppKit)
    let url = URL(fileURLWithPath: path)
    guard let service = NSSharingService(named: .cloudSharing),
        service.canPerform(withItems: [url])
    else {
        emit(["type": "error", "message": "this Mac can't share that folder from here"])
        return
    }
    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)
    let delegate = ShareDelegate()
    service.delegate = delegate
    app.activate(ignoringOtherApps: true)
    service.perform(withItems: [url])
    // The sheet is up; the caller stops waiting on us here and the person
    // finishes in macOS's own UI. A sheet nobody answers is not left running
    // forever: ten minutes and this process is gone, share or no share.
    emit(["type": "presented"])
    DispatchQueue.main.asyncAfter(deadline: .now() + 600) { exit(0) }
    app.run()
    #else
    emit(["type": "error", "message": "the share sheet needs macOS"])
    #endif
}

let arguments = CommandLine.arguments
if arguments.contains("--probe") {
    probe()
} else if let flag = arguments.firstIndex(of: "--share"), flag + 1 < arguments.count {
    share(arguments[flag + 1])
} else if arguments.contains("--share") {
    emit(["type": "error", "message": "--share needs a folder"])
} else {
    await respond()
}
