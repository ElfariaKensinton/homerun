import SwiftUI

struct SettingsView: View {
    @EnvironmentObject private var model: AppModel

    @State private var startRunnersOnLaunch = false
    @State private var notifyStatusChanges = true
    @State private var notifyJobCompletions = true
    @State private var autoScan = false
    @State private var watchNewRepositories = false
    @State private var autoCreateRunnerForNewRepositories = false
    @State private var scanLabels = "self-hosted"
    @State private var workspacePath = ""
    @State private var customRunnerDownloadUrl = ""
    @State private var artifactResultsUrl = ""
    @State private var pat = ""
    @State private var deviceFlow: DeviceFlowResponse?
    @State private var working = false
    @State private var message: String?
    @State private var errorMessage: String?
    @State private var confirmShutdown = false
    @State private var confirmUninstall = false

    var body: some View {
        Form {
            Section("GitHub authentication") {
                if model.authenticated {
                    Label("Signed in as @\(model.signedInUser)", systemImage: "checkmark.circle.fill")
                        .foregroundStyle(.green)
                    Button("Sign out of GitHub", role: .destructive) {
                        Task {
                            do {
                                try await model.signOutFromGitHub()
                                message = "Signed out of GitHub."
                            } catch { errorMessage = error.localizedDescription }
                        }
                    }
                } else {
                    Button {
                        Task {
                            do {
                                deviceFlow = try await model.startDeviceFlow()
                                message = "Open the verification page and enter the code below."
                            } catch { errorMessage = error.localizedDescription }
                        }
                    } label: {
                        Label("Sign in with GitHub Device Flow", systemImage: "person.badge.key")
                    }
                    SecureField("Personal access token (optional)", text: $pat)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    Button("Sign in with token") {
                        Task {
                            do {
                                try await model.signInWithToken(pat)
                                pat = ""
                                message = "GitHub authentication succeeded."
                                errorMessage = nil
                            } catch { errorMessage = error.localizedDescription }
                        }
                    }
                    .disabled(pat.isEmpty || working)
                    Text("Device Flow is preferred. If you use a PAT, it is sent to the daemon and is not persisted by this app.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }

                if let deviceFlow {
                    VStack(alignment: .leading, spacing: 10) {
                        Text("Enter this device code").font(.subheadline)
                        Text(deviceFlow.userCode)
                            .font(.system(size: 26, weight: .bold, design: .monospaced))
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity)
                            .padding()
                            .background(.quaternary, in: RoundedRectangle(cornerRadius: 12))
                        Link(destination: URL(string: deviceFlow.verificationUri)!) {
                            Label("Open GitHub verification", systemImage: "arrow.up.right.square")
                        }
                        Button(working ? "Checking authorization…" : "I've approved access — check now") {
                            Task { await pollDeviceFlow(deviceFlow) }
                        }
                        .disabled(working)
                        Text("The code expires in about \(max(1, deviceFlow.expiresIn / 60)) minutes.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
            }

            Section("Runner behavior") {
                Toggle("Start runners on daemon launch", isOn: $startRunnersOnLaunch)
                Toggle("Notify on runner status changes", isOn: $notifyStatusChanges)
                Toggle("Notify on job completion", isOn: $notifyJobCompletions)
                Toggle("Scan workspace automatically", isOn: $autoScan)
                Toggle("Watch for new GitHub repositories", isOn: $watchNewRepositories)
                Toggle("Auto-create runners for new repositories", isOn: $autoCreateRunnerForNewRepositories)
                TextField("Scan labels (comma-separated)", text: $scanLabels)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                TextField("Host workspace path", text: $workspacePath)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                TextField("Custom runner archive URL", text: $customRunnerDownloadUrl)
                    .keyboardType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                TextField("Artifact results API URL", text: $artifactResultsUrl)
                    .keyboardType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                Button {
                    Task { await savePreferences() }
                } label: {
                    HStack {
                        Spacer()
                        if working { ProgressView().padding(.trailing, 6) }
                        Text("Save preferences")
                            .fontWeight(.semibold)
                        Spacer()
                    }
                }
                .disabled(working)
            }

            Section("Host daemon") {
                HStack {
                    Text("Daemon version")
                    Spacer()
                    Text(model.daemonVersion.isEmpty ? "—" : model.daemonVersion)
                        .foregroundStyle(.secondary)
                }
                HStack {
                    Text("Docker runner support")
                    Spacer()
                    Text(model.dockerStatus?.available == true ? "Available" : (model.dockerStatus == nil ? "Unknown" : "Unavailable"))
                        .foregroundStyle(model.dockerStatus?.available == true ? .green : .secondary)
                }
                if let dockerError = model.dockerStatus?.error, !dockerError.isEmpty {
                    Text(dockerError)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                HStack {
                    Text("Background service")
                    Spacer()
                    Text(serviceText)
                        .foregroundStyle(.secondary)
                }
                Button {
                    Task {
                        do { try await model.loadServiceStatus() }
                        catch { errorMessage = error.localizedDescription }
                    }
                } label: {
                    Label("Refresh service status", systemImage: "arrow.clockwise")
                }
                Button {
                    Task {
                        do {
                            try await model.serviceAction("install")
                            message = "Service install request completed."
                        } catch { errorMessage = error.localizedDescription }
                    }
                } label: {
                    Label("Install background service", systemImage: "gearshape.2")
                }
                Button(role: .destructive) { confirmUninstall = true } label: {
                    Label("Uninstall background service", systemImage: "trash")
                }
                Button {
                    Task {
                        do {
                            try await model.checkForUpdates()
                            message = model.updateStatus?.updateAvailable == true ? "Update available: \(model.updateStatus?.latest ?? "unknown")" : "HomeRun is up to date."
                        } catch { errorMessage = error.localizedDescription }
                    }
                } label: {
                    Label("Check for updates", systemImage: "arrow.down.circle")
                }
                if let update = model.updateStatus {
                    LabeledContent("Current version", value: update.current ?? model.daemonVersion)
                    LabeledContent("Latest version", value: update.latest ?? "Unavailable")
                    if update.updateAvailable {
                        Text("A newer version is available on the host. Updating remains a host-side operation.")
                            .font(.footnote)
                            .foregroundStyle(.orange)
                    }
                }
                Button(role: .destructive) { confirmShutdown = true } label: {
                    Label("Shut down daemon", systemImage: "power")
                }
                Text("Shutdown stops active runners and disconnects this app. If the daemon is managed by a service, its own policy may prevent shutdown.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }

            Section("Connection") {
                LabeledContent("Endpoint", value: model.endpoint)
                Text("The remote API token is kept in the iOS Keychain. The daemon-side bridge must remain loopback-bound and be reached through a trusted HTTPS tunnel.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                Button("Disconnect and forget token", role: .destructive) {
                    model.forgetConnection()
                }
            }

            if let message {
                Section {
                    Label(message, systemImage: "info.circle")
                        .font(.footnote)
                }
            }
            if let errorMessage {
                Section {
                    Label(errorMessage, systemImage: "exclamationmark.triangle")
                        .font(.footnote)
                        .foregroundStyle(.red)
                }
            }

            Section {
                HStack {
                    Text("HomeRun for iOS")
                    Spacer()
                    Text("Native · iOS 17+")
                        .foregroundStyle(.secondary)
                }
                Link("Repository and documentation", destination: URL(string: "https://github.com/ElfariaKensinton/homerun/tree/feat/ios-app/ios")!)
            }
        }
        .navigationTitle("Settings")
        .task {
            await model.refresh()
            loadPreferences(model.preferences)
            try? await model.loadServiceStatus()
        }
        .confirmationDialog("Uninstall the host daemon service?", isPresented: $confirmUninstall, titleVisibility: .visible) {
            Button("Uninstall service", role: .destructive) {
                Task {
                    do {
                        try await model.serviceAction("uninstall")
                        message = "Service uninstall request completed."
                    } catch { errorMessage = error.localizedDescription }
                }
            }
        } message: {
            Text("This changes service-manager configuration on the computer running HomeRun.")
        }
        .confirmationDialog("Shut down HomeRun daemon?", isPresented: $confirmShutdown, titleVisibility: .visible) {
            Button("Shut down daemon", role: .destructive) {
                Task {
                    do { try await model.shutdownDaemon() }
                    catch { errorMessage = error.localizedDescription }
                }
            }
        } message: {
            Text("Active runners are stopped before shutdown. You will need to start the daemon again on the host.")
        }
        .alert("Settings", isPresented: Binding(
            get: { errorMessage != nil },
            set: { if !$0 { errorMessage = nil } }
        )) {
            Button("OK", role: .cancel) { errorMessage = nil }
        } message: {
            Text(errorMessage ?? "")
        }
    }

    private var serviceText: String {
        guard let service = model.serviceStatus else { return "Unknown" }
        return service.installed ? "Installed" : "Not installed"
    }

    private func loadPreferences(_ value: Preferences?) {
        guard let value else { return }
        startRunnersOnLaunch = value.startRunnersOnLaunch
        notifyStatusChanges = value.notifyStatusChanges
        notifyJobCompletions = value.notifyJobCompletions
        autoScan = value.autoScan
        watchNewRepositories = value.watchNewRepositories
        autoCreateRunnerForNewRepositories = value.autoCreateRunnerForNewRepositories
        scanLabels = value.scanLabels.joined(separator: ", ")
        workspacePath = value.workspacePath ?? ""
        customRunnerDownloadUrl = value.customRunnerDownloadUrl ?? ""
        artifactResultsUrl = value.artifactResultsUrl ?? ""
    }

    private func savePreferences() async {
        working = true
        defer { working = false }
        var value = model.preferences ?? Preferences()
        value.startRunnersOnLaunch = startRunnersOnLaunch
        value.notifyStatusChanges = notifyStatusChanges
        value.notifyJobCompletions = notifyJobCompletions
        value.autoScan = autoScan
        value.watchNewRepositories = watchNewRepositories
        value.autoCreateRunnerForNewRepositories = autoCreateRunnerForNewRepositories
        value.scanLabels = scanLabels.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        value.workspacePath = workspacePath.isEmpty ? nil : workspacePath
        value.customRunnerDownloadUrl = customRunnerDownloadUrl.isEmpty ? nil : customRunnerDownloadUrl
        value.artifactResultsUrl = artifactResultsUrl.isEmpty ? nil : artifactResultsUrl
        do {
            try await model.updatePreferences(value)
            message = "Preferences saved."
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func pollDeviceFlow(_ flow: DeviceFlowResponse) async {
        working = true
        defer { working = false }
        message = "Waiting for GitHub authorization…"
        let attempts = max(1, min(180, flow.expiresIn / max(1, flow.interval)))
        for _ in 0..<attempts {
            if Task.isCancelled { return }
            do {
                let status = try await model.pollDeviceFlow(flow)
                if status.authenticated {
                    deviceFlow = nil
                    message = "GitHub authentication succeeded."
                    errorMessage = nil
                    return
                }
            } catch {
                // The daemon uses HTTP 401 while the device grant is still pending.
            }
            try? await Task.sleep(nanoseconds: UInt64(max(1, flow.interval)) * 1_000_000_000)
        }
        message = "Device authorization timed out. Start a new sign-in flow."
        deviceFlow = nil
    }
}
