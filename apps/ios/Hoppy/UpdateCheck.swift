// "Hoppy X disponibile": at most every 3 hours, when the app comes to the foreground (and at
// once from the menu item "Controlla aggiornamenti"), ask GitHub for the
// latest release (the same `releases/latest` contract as the other platforms, docs/release.md)
// and announce it when it carries Hoppy-ios-unsigned.ipa and is newer than this build. No
// download: a sideloaded app is replaced by SideStore (source on the portal) or Sideloadly.
import Foundation

enum UpdateCheck {
    static let latestAPI = URL(string: "https://api.github.com/repos/Marketto/hoppy/releases/latest")!
    static let portal = URL(string: "https://marketto.github.io/hoppy/")!
    static let asset = "Hoppy-ios-unsigned.ipa"
    static let every: TimeInterval = 3 * 3600

    private static let checkedKey = "hoppy.update.checked"
    private static let latestKey = "hoppy.update.latest"
    private static let dismissedKey = "hoppy.update.dismissed"

    static var current: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0"
    }

    /// The version to announce now (from the last check), nil if none or dismissed.
    static var pending: String? {
        let d = UserDefaults.standard
        guard let latest = d.string(forKey: latestKey), isNewer(latest, than: current),
              d.string(forKey: dismissedKey) != latest else { return nil }
        return latest
    }

    static func dismiss(_ version: String) {
        UserDefaults.standard.set(version, forKey: dismissedKey)
    }

    /// Answer of `checkNow`.
    enum Outcome {
        case upToDate(String)
        case available(String)
        case failed(String)
    }

    /// Checks if the last check is older than `every` (3 h); `done` runs on the main queue
    /// with `pending` (also when no check was due).
    static func runIfDue(_ done: @escaping (String?) -> Void) {
        let d = UserDefaults.standard
        if let last = d.object(forKey: checkedKey) as? Date, Date().timeIntervalSince(last) >= 0,
           Date().timeIntervalSince(last) < every {
            return done(pending)
        }
        fetch { _ in done(pending) }
    }

    /// Menu "Controlla aggiornamenti": checks now, ignoring `every`, and always answers on the
    /// main queue. A version the user had dismissed is announced again (they asked).
    static func checkNow(_ done: @escaping (Outcome) -> Void) {
        fetch { result in
            switch result {
            case .failure(let error):
                done(.failed(error.localizedDescription))
            case .success(let latest):
                if let latest = latest, isNewer(latest, than: current) {
                    UserDefaults.standard.removeObject(forKey: dismissedKey)
                    done(.available(latest))
                } else {
                    done(.upToDate(current))
                }
            }
        }
    }

    private struct HTTPStatus: LocalizedError {
        let code: Int
        var errorDescription: String? { "HTTP \(code)" }
    }

    /// One request to `releases/latest`; stores the result only after a complete response
    /// (errors retry on the next foreground). `done` runs on the main queue.
    private static func fetch(_ done: @escaping (Result<String?, Error>) -> Void) {
        let d = UserDefaults.standard
        var request = URLRequest(url: latestAPI, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 20)
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        request.setValue("Hoppy-iOS/\(current)", forHTTPHeaderField: "User-Agent")
        URLSession.shared.dataTask(with: request) { data, response, error in
            let code = (response as? HTTPURLResponse)?.statusCode ?? 0
            let latest = code == 200 ? data.flatMap { UpdateCheck.parse($0) } : nil
            DispatchQueue.main.async {
                guard code == 200 else {
                    return done(.failure(error ?? HTTPStatus(code: code)))
                }
                d.set(Date(), forKey: UpdateCheck.checkedKey)
                d.set(latest, forKey: UpdateCheck.latestKey)
                done(.success(latest))
            }
        }.resume()
    }

    /// `tag_name` without the "v", only when the release carries the iOS asset.
    static func parse(_ data: Data) -> String? {
        struct Release: Decodable {
            struct Asset: Decodable { let name: String }
            let tag_name: String
            let assets: [Asset]
        }
        guard let r = try? JSONDecoder().decode(Release.self, from: data),
              r.assets.contains(where: { $0.name == asset }) else { return nil }
        let v = r.tag_name.hasPrefix("v") ? String(r.tag_name.dropFirst()) : r.tag_name
        return numbers(v) == nil ? nil : v
    }

    /// `x.y.z` (a `-suffix` is ignored, as build-ipa.sh does for the bundle version).
    static func numbers(_ v: String) -> [Int]? {
        let core = v.split(separator: "-", maxSplits: 1).first.map(String.init) ?? v
        let parts = core.split(separator: ".", omittingEmptySubsequences: false).map { Int($0) }
        guard parts.count == 3, parts.allSatisfy({ $0 != nil }) else { return nil }
        return parts.compactMap { $0 }
    }

    static func isNewer(_ a: String, than b: String) -> Bool {
        guard let x = numbers(a) else { return false }
        guard let y = numbers(b) else { return true }
        return x.lexicographicallyPrecedes(y) == false && x != y
    }
}
