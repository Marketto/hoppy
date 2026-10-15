import SwiftUI

@main
struct HoppyApp: App {
    static func isInOwnInbox(_ url: URL) -> Bool {
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let inbox = docs.appendingPathComponent("Inbox", isDirectory: true)
            .standardizedFileURL.resolvingSymlinksInPath().path
        let opened = url.standardizedFileURL.resolvingSymlinksInPath().path
        return opened.hasPrefix(inbox + "/")
    }

    @StateObject private var model = AppModel()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(model)
                // "Open in Hoppy" / Files → Share → Hoppy (document types, Info.plist).
                .onOpenURL { url in
                    guard url.isFileURL else { return }
                    if let staged = try? Outbox.stage(url) {
                        model.offerToSend([staged])
                    }
                    // A copy iOS put into our own Documents/Inbox is ours to delete; a file
                    // opened in place (LSSupportsOpeningDocumentsInPlace) never is.
                    if Self.isInOwnInbox(url) { try? FileManager.default.removeItem(at: url) }
                }
        }
        .onChange(of: scenePhase) { phase in
            switch phase {
            case .active: model.enterForeground()
            case .background: model.enterBackground()
            default: break
            }
        }
    }
}
