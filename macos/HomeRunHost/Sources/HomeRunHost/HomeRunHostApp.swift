import AppKit
import Combine
import Security
import SwiftUI

@main
struct HomeRunHostApp: App {
    @NSApplicationDelegateAdaptor(AppLifecycle.self) private var appLifecycle
    @StateObject private var manager = HostManager()

    var body: some Scene {
        WindowGroup {
            HostView(manager: manager)
                .frame(minWidth: 680, minHeight: 640)
                .onAppear {
                    appLifecycle.manager = manager
                }
        }
        .commands {
            CommandGroup(replacing: .newItem) {}
        }
    }
}

final class AppLifecycle: NSObject, NSApplicationDelegate {
    weak var manager: HostManager?

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }

    func applicationWillTerminate(_ notification: Notification) {
        manager?.stop()
    }
}

@MainActor
final class HostManager: ObservableObject {
    static let port = 47831

    @Published private(set) var isRunning = false
    @Published private(set) var isStarting = false
    @Published private(set) var statusMessage = "Not running"
    @Published private(set) var logText = "HomeRun Host is ready. Start the daemon to begin."
    @Published private(set) var tailscaleURL: String?
    @Published private(set) var tailscaleMessage = "Private remote access is not configured."
    @Published private(set) var isConfiguringTailscale = false
    @Published var revealsToken = false

    let token = HostToken.loadOrCreate()

    private var daemon: Process?
    private var outputPipe: Pipe?

    func start() {
        guard daemon?.isRunning != true, !isStarting else { return }

        guard let executable = Bundle.main.resourceURL?.appendingPathComponent("homerund"),
              FileManager.default.isExecutableFile(atPath: executable.path) else {
            statusMessage = "Daemon binary missing"
            appendLog("Could not find the bundled homerund executable. Reinstall the HomeRun Host app.")
            return
        }

        let child = Process()
        child.executableURL = executable
        var environment = ProcessInfo.processInfo.environment
        environment["HOMERUN_REMOTE_BIND"] = "127.0.0.1:\(Self.port)"
        environment["HOMERUN_REMOTE_TOKEN"] = token
        child.environment = environment

        let pipe = Pipe()
        child.standardOutput = pipe
        child.standardError = pipe
        pipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            guard !data.isEmpty else {
                handle.readabilityHandler = nil
                return
            }
            let text = String(decoding: data, as: UTF8.self)
            DispatchQueue.main.async {
                self?.appendLog(text)
            }
        }

        child.terminationHandler = { [weak self] finishedProcess in
            DispatchQueue.main.async {
                guard let self else { return }
                self.isRunning = false
                self.isStarting = false
                self.statusMessage = "Stopped (exit \(finishedProcess.terminationStatus))"
                self.appendLog("\nDaemon exited with code \(finishedProcess.terminationStatus).\n")
                self.daemon = nil
                self.outputPipe?.fileHandleForReading.readabilityHandler = nil
                self.outputPipe = nil
            }
        }

        do {
            try child.run()
            daemon = child
            outputPipe = pipe
            isRunning = true
            isStarting = false
            statusMessage = "Running on 127.0.0.1:\(Self.port)"
            appendLog("\nStarting homerund with authenticated loopback access on port \(Self.port).\n")
        } catch {
            statusMessage = "Failed to start"
            appendLog("Failed to launch homerund: \(error.localizedDescription)\n")
        }
    }

    func stop() {
        guard let process = daemon, process.isRunning else { return }
        statusMessage = "Stopping…"
        appendLog("\nStopping homerund…\n")
        process.terminate()
    }

    func copyToken() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(token, forType: .string)
    }

    func copyRemoteURL() {
        guard let tailscaleURL else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(tailscaleURL, forType: .string)
    }

    func enableTailscaleServe() {
        guard isRunning, !isConfiguringTailscale else { return }
        isConfiguringTailscale = true
        tailscaleMessage = "Configuring Tailscale Serve…"

        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self else { return }
            guard let binary = Self.findTailscale() else {
                DispatchQueue.main.async {
                    self.isConfiguringTailscale = false
                    self.tailscaleMessage = "Tailscale CLI not found. Install and sign in to Tailscale, then try again."
                }
                return
            }

            let configure = Self.runCommand(binary, arguments: [
                "serve", "--bg", "http://127.0.0.1:\(Self.port)"
            ])
            let status = Self.runCommand(binary, arguments: ["serve", "status"])
            let combined = [configure.output, status.output].filter { !$0.isEmpty }.joined(separator: "\n")
            let url = Self.extractServeURL(combined)

            DispatchQueue.main.async {
                self.isConfiguringTailscale = false
                if configure.exitCode == 0, let url {
                    self.tailscaleURL = url
                    self.tailscaleMessage = "Available only to devices in your Tailscale network."
                    self.appendLog("Tailscale Serve enabled for private HTTPS access: \(url)\n")
                } else {
                    self.tailscaleMessage = combined.isEmpty
                        ? "Tailscale Serve failed (exit \(configure.exitCode)). Check that Tailscale is running and HTTPS is enabled for your tailnet."
                        : combined
                    self.appendLog("Tailscale Serve could not be confirmed. \(combined)\n")
                }
            }
        }
    }

    private func appendLog(_ text: String) {
        logText += text
        if logText.count > 24_000 {
            logText = String(logText.suffix(24_000))
        }
    }

    nonisolated private static func findTailscale() -> String? {
        let candidates = [
            "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
            "/opt/homebrew/bin/tailscale",
            "/usr/local/bin/tailscale"
        ]
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0) }
    }

    nonisolated private static func runCommand(_ path: String, arguments: [String]) -> (exitCode: Int32, output: String) {
        let process = Process()
        let pipe = Pipe()
        process.executableURL = URL(fileURLWithPath: path)
        process.arguments = arguments
        process.standardOutput = pipe
        process.standardError = pipe
        process.standardInput = FileHandle.nullDevice

        do {
            try process.run()
            process.waitUntilExit()
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            return (process.terminationStatus, String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines))
        } catch {
            return (-1, error.localizedDescription)
        }
    }

    nonisolated private static func extractServeURL(_ text: String) -> String? {
        for part in text.components(separatedBy: .whitespacesAndNewlines) {
            let candidate = part.trimmingCharacters(in: CharacterSet(charactersIn: "|[](),"))
            if let url = URL(string: candidate),
               url.scheme == "https",
               let host = url.host,
               host.hasSuffix(".ts.net") {
                return url.absoluteString.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            }
        }
        return nil
    }
}

enum HostToken {
    private static let service = "com.elfariakensinton.homerun-host"
    private static let account = "remote-api-token"

    static func loadOrCreate() -> String {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne
        ]
        var result: CFTypeRef?
        if SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
           let data = result as? Data,
           let token = String(data: data, encoding: .utf8),
           !token.isEmpty {
            return token
        }

        var bytes = [UInt8](repeating: 0, count: 32)
        guard SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess else {
            return UUID().uuidString.replacingOccurrences(of: "-", with: "") + UUID().uuidString.replacingOccurrences(of: "-", with: "")
        }
        let token = bytes.map { String(format: "%02x", $0) }.joined()
        let record: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecValueData as String: Data(token.utf8),
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        ]
        let status = SecItemAdd(record as CFDictionary, nil)
        if status == errSecDuplicateItem {
            return loadOrCreate()
        }
        return token
    }
}

struct HostView: View {
    @ObservedObject var manager: HostManager

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(spacing: 12) {
                Image(systemName: "house.and.flag.fill")
                    .font(.system(size: 30))
                    .foregroundStyle(.tint)
                VStack(alignment: .leading, spacing: 3) {
                    Text("HomeRun Host")
                        .font(.largeTitle.bold())
                    Text("Apple Silicon · macOS 13+")
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Label(manager.isRunning ? "Running" : "Stopped",
                      systemImage: manager.isRunning ? "checkmark.circle.fill" : "circle")
                    .foregroundColor(manager.isRunning ? .green : .secondary)
                    .font(.headline)
            }

            GroupBox("Daemon") {
                VStack(alignment: .leading, spacing: 12) {
                    HStack {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(manager.statusMessage)
                                .font(.headline)
                            Text("Authenticated local API · 127.0.0.1:\(HostManager.port)")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button {
                            manager.start()
                        } label: {
                            Label("Start", systemImage: "play.fill")
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(manager.isRunning)

                        Button {
                            manager.stop()
                        } label: {
                            Label("Stop", systemImage: "stop.fill")
                        }
                        .buttonStyle(.bordered)
                        .disabled(!manager.isRunning)
                    }
                    Text("The daemon runs as a child of this app. Quitting HomeRun Host stops it.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .padding(6)
            }

            GroupBox("iPhone connection") {
                VStack(alignment: .leading, spacing: 12) {
                    Label("Private HTTPS access", systemImage: "lock.shield")
                        .font(.headline)
                    Text("Enable Tailscale Serve to connect your iPhone over your tailnet. This does not expose the daemon publicly.")
                        .foregroundStyle(.secondary)

                    HStack {
                        Button {
                            manager.enableTailscaleServe()
                        } label: {
                            if manager.isConfiguringTailscale {
                                ProgressView().controlSize(.small)
                                Text("Configuring…")
                            } else {
                                Label("Enable Tailscale Serve", systemImage: "network")
                            }
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(!manager.isRunning || manager.isConfiguringTailscale)

                        if let url = manager.tailscaleURL {
                            Button("Copy URL") {
                                manager.copyRemoteURL()
                            }
                            .buttonStyle(.bordered)
                            .help(url)
                        }
                    }

                    Text(manager.tailscaleMessage)
                        .font(.caption)
                        .textSelection(.enabled)

                    Divider()

                    HStack(alignment: .center) {
                        VStack(alignment: .leading, spacing: 5) {
                            Text("API token")
                                .font(.headline)
                            Text(manager.revealsToken ? manager.token : String(repeating: "•", count: 48))
                                .font(.system(.caption, design: .monospaced))
                                .textSelection(.enabled)
                        }
                        Spacer()
                        Button(manager.revealsToken ? "Hide" : "Show") {
                            manager.revealsToken.toggle()
                        }
                        Button {
                            manager.copyToken()
                        } label: {
                            Label("Copy token", systemImage: "doc.on.doc")
                        }
                        .buttonStyle(.bordered)
                    }

                    Text("Enter the HTTPS URL and this token in the HomeRun iOS app. The token is stored in the macOS Keychain. Keep it private.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .padding(6)
            }

            GroupBox("Daemon log") {
                ScrollView {
                    Text(manager.logText)
                        .font(.system(.caption, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(8)
                }
                .frame(minHeight: 140, maxHeight: .infinity)
            }

            HStack {
                Spacer()
                Text("HomeRun · local daemon manager")
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
        }
        .padding(22)
    }
}
