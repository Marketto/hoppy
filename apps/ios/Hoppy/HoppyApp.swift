import SwiftUI

@main
struct HoppyApp: App {
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
                    // Files iOS copied into Documents/Inbox are ours to delete.
                    if url.path.contains("/Documents/Inbox/") { try? FileManager.default.removeItem(at: url) }
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
