import Foundation
import Combine

@MainActor
final class AppModel: ObservableObject {
    @Published var endpoint = UserDefaults.standard.string(forKey: "homerun.endpoint") ?? ""
    @Published var isConnected = false
    @Published var isRefreshing = false
    @Published var lastError: String?
    @Published var daemonVersion = ""
    @Published var runners: [RunnerInfo] = []
    @Published var repositories: [RepoInfo] = []
    @Published var metrics: MetricsResponse?
    @Published var authStatus: AuthStatus?
    @Published var preferences: Preferences?
    @Published var dockerStatus: DockerStatusResponse?
    @Published var scanResults: ScanResults?
    @Published var daemonLogs: [DaemonLogEntry] = []
    @Published var serviceStatus: ServiceStatusResponse?
    @Published var updateStatus: UpdateCheckResponse?
    @Published var activeScan = false
    @Published var activeScanId: String?
    @Published var scanProgressText: String?

    private(set) var api: HomeRunAPI?
    private var pollTask: Task<Void, Never>?
    private var eventsTask: Task<Void, Never>?

    var authenticated: Bool { authStatus?.authenticated == true }
    var signedInUser: String { authStatus?.user?.login ?? "Not signed in" }

    func restoreConnection() async {
        guard !endpoint.isEmpty, let token = SecureStore.readToken(), !token.isEmpty else { return }
        do {
            try await connect(endpoint: endpoint, token: token, persist: false)
        } catch {
            lastError = error.localizedDescription
        }
    }

    func connect(endpoint rawEndpoint: String, token rawToken: String, persist: Bool = true) async throws {
        let token = rawToken.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !token.isEmpty else { throw HomeRunAPIError.missingConnection }
        let client = try HomeRunAPI(endpoint: rawEndpoint, token: token)
        let health = try await client.health()

        stopBackgroundTasks()
        api = client
        endpoint = client.baseUrl.absoluteString
        daemonVersion = health.version ?? ""
        isConnected = true
        lastError = nil
        if persist {
            try SecureStore.saveToken(token)
            UserDefaults.standard.set(endpoint, forKey: "homerun.endpoint")
        }
        await refresh()
        startBackgroundTasks()
    }

    func forgetConnection() {
        stopBackgroundTasks()
        api = nil
        isConnected = false
        endpoint = ""
        runners = []
        repositories = []
        metrics = nil
        authStatus = nil
        preferences = nil
        dockerStatus = nil
        scanResults = nil
        daemonLogs = []
        SecureStore.deleteToken()
        UserDefaults.standard.removeObject(forKey: "homerun.endpoint")
    }

    private func startBackgroundTasks() {
        pollTask?.cancel()
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 2_000_000_000)
                guard let self, !Task.isCancelled else { return }
                await self.refresh()
            }
        }

        eventsTask?.cancel()
        eventsTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self, let client = self.api else { return }
                do {
                    for try await _ in client.events() {
                        guard !Task.isCancelled else { return }
                        await self.refresh()
                    }
                } catch {
                    // Polling remains the fallback when WebSocket events are unavailable.
                }
                try? await Task.sleep(nanoseconds: 2_000_000_000)
            }
        }
    }

    private func stopBackgroundTasks() {
        pollTask?.cancel()
        eventsTask?.cancel()
        pollTask = nil
        eventsTask = nil
    }

    func refresh() async {
        guard let api else { return }
        isRefreshing = true
        defer { isRefreshing = false }

        do {
            let health = try await api.health()
            daemonVersion = health.version ?? daemonVersion
        } catch {
            lastError = error.localizedDescription
            isConnected = false
            return
        }

        if let value = try? await api.get("runners", as: [RunnerInfo].self) {
            runners = value
        }
        if let value = try? await api.get("auth/status", as: AuthStatus.self) {
            authStatus = value
            if value.authenticated {
                if let repos = try? await api.get("repos", as: [RepoInfo].self) {
                    repositories = repos
                }
            } else {
                repositories = []
            }
        }
        if let value = try? await api.get("metrics", as: MetricsResponse.self) {
            metrics = value
        }
        if let value = try? await api.get("preferences", as: Preferences.self) {
            preferences = value
        }
        if let value = try? await api.get("system/docker-status", as: DockerStatusResponse.self) {
            dockerStatus = value
        }
        lastError = nil
    }

    func metric(for runnerId: String) -> RunnerMetrics? {
        metrics?.runners.first(where: { $0.runnerId == runnerId })
    }

    func loadDaemonLogs(level: String? = nil, limit: Int = 500, search: String? = nil) async throws {
        guard let api else { throw HomeRunAPIError.missingConnection }
        var query = [URLQueryItem(name: "limit", value: String(limit))]
        if let level, level != "All" { query.append(URLQueryItem(name: "level", value: level.lowercased())) }
        if let search, !search.isEmpty { query.append(URLQueryItem(name: "search", value: search)) }
        daemonLogs = try await api.get("daemon/logs/recent", query: query, as: [DaemonLogEntry].self)
    }

    func daemonLogsStream(level: String?) -> AsyncThrowingStream<DaemonLogEntry, Error>? {
        api?.streamDaemonLogs(level: level)
    }

    func appendDaemonLog(_ entry: DaemonLogEntry) {
        guard !daemonLogs.contains(where: { $0.id == entry.id }) else { return }
        daemonLogs.insert(entry, at: 0)
        if daemonLogs.count > 2_000 {
            daemonLogs.removeLast(daemonLogs.count - 2_000)
        }
    }

    func loadScanResults() async throws {
        guard let api else { throw HomeRunAPIError.missingConnection }
        scanResults = try await api.get("scan/results", as: ScanResults?.self)
    }

    func scanRemote() async throws {
        guard let api else { throw HomeRunAPIError.missingConnection }
        let scanId = UUID().uuidString
        activeScanId = scanId
        activeScan = true
        scanProgressText = "Starting GitHub scan…"
        defer {
            activeScan = false
            activeScanId = nil
            scanProgressText = nil
        }

        let stream = api.streamScan(path: "scan/remote/stream", scanId: scanId, body: EmptyPayload())
        for try await event in stream {
            guard !Task.isCancelled else { return }
            updateScanProgress(event)
        }
        try await loadScanResults()
        await refresh()
    }

    func scanLocal(path: String) async throws {
        guard let api else { throw HomeRunAPIError.missingConnection }
        let scanId = UUID().uuidString
        activeScanId = scanId
        activeScan = true
        scanProgressText = "Starting host workspace scan…"
        defer {
            activeScan = false
            activeScanId = nil
            scanProgressText = nil
        }

        let request = ScanStreamLocalRequest(path: path)
        let stream = api.streamScan(path: "scan/local/stream", scanId: scanId, body: request)
        for try await event in stream {
            guard !Task.isCancelled else { return }
            updateScanProgress(event)
        }
        try await loadScanResults()
    }

    func cancelScan() async throws {
        guard let api, let scanId = activeScanId else { return }
        let _: ScanCancelResponse = try await api.post(
            "scan/cancel",
            body: CancelScanRequest(scanId: scanId),
            as: ScanCancelResponse.self
        )
        scanProgressText = "Cancellation requested…"
    }

    private func updateScanProgress(_ event: ScanProgressEvent) {
        switch event.type {
        case "started":
            scanProgressText = "Scanning \(event.scanType ?? "repositories") · \(event.total ?? 0) to check"
        case "checking":
            scanProgressText = "Checking \(event.repo ?? "repository") (\(event.index ?? 0)/\(event.total ?? 0))"
        case "found":
            scanProgressText = "Found \(event.fullName ?? "a matching repository")"
        case "done":
            scanProgressText = "Finished · \(event.totalFound ?? 0) found · \(event.totalChecked ?? 0) checked"
        case "cancelled":
            scanProgressText = "Cancelled · \(event.checked ?? 0)/\(event.total ?? 0) checked"
        default:
            scanProgressText = "Scanning repositories…"
        }
    }

    func createRunner(_ request: CreateRunnerRequest) async throws -> RunnerInfo {
        guard let api else { throw HomeRunAPIError.missingConnection }
        let created = try await api.post("runners", body: request, as: RunnerInfo.self)
        await refresh()
        return created
    }

    func createBatch(_ request: CreateBatchRequest) async throws -> BatchCreateResponse {
        guard let api else { throw HomeRunAPIError.missingConnection }
        let result = try await api.post("runners/batch", body: request, as: BatchCreateResponse.self)
        await refresh()
        return result
    }

    func runnerAction(_ id: String, action: String) async throws {
        guard let api else { throw HomeRunAPIError.missingConnection }
        try await api.command("runners/\(id)/\(action)")
        await refresh()
    }

    func updateRunner(_ id: String, labels: [String], mode: String) async throws {
        guard let api else { throw HomeRunAPIError.missingConnection }
        let request = UpdateRunnerRequest(labels: labels, mode: mode)
        let _: RunnerInfo = try await api.patch("runners/\\(id)", body: request, as: RunnerInfo.self)
        await refresh()
    }

    func deleteRunner(_ id: String) async throws {
        guard let api else { throw HomeRunAPIError.missingConnection }
        try await api.command("runners/\(id)", method: "DELETE")
        await refresh()
    }

    func groupAction(_ groupId: String, action: String) async throws -> GroupActionResponse {
        guard let api else { throw HomeRunAPIError.missingConnection }
        let result = try await api.post("runners/groups/\(groupId)/\(action)", as: GroupActionResponse.self)
        await refresh()
        return result
    }

    func deleteGroup(_ groupId: String) async throws -> GroupActionResponse {
        guard let api else { throw HomeRunAPIError.missingConnection }
        try await api.command("runners/groups/\(groupId)", method: "DELETE")
        await refresh()
        return GroupActionResponse(groupId: groupId, results: [])
    }

    func scaleGroup(_ groupId: String, count: Int) async throws -> ScaleGroupResponse {
        guard let api else { throw HomeRunAPIError.missingConnection }
        let result = try await api.patch("runners/groups/\(groupId)", body: ScaleRequest(count: count), as: ScaleGroupResponse.self)
        await refresh()
        return result
    }

    func loadRunnerLogs(_ id: String) async throws -> [LogEntry] {
        guard let api else { throw HomeRunAPIError.missingConnection }
        return try await api.get("runners/\(id)/logs/recent", as: [LogEntry].self)
    }

    func logsStream(for id: String) -> AsyncThrowingStream<LogEntry, Error>? {
        api?.streamLogs(runnerId: id)
    }

    func loadSteps(_ id: String) async throws -> StepsResponse {
        guard let api else { throw HomeRunAPIError.missingConnection }
        return try await api.get("runners/\(id)/steps", as: StepsResponse.self)
    }

    func loadStepLogs(_ id: String, step: Int) async throws -> StepLogsResponse {
        guard let api else { throw HomeRunAPIError.missingConnection }
        return try await api.get("runners/\(id)/steps/\(step)/logs", as: StepLogsResponse.self)
    }

    func loadHistory(_ id: String) async throws -> [JobHistoryEntry] {
        guard let api else { throw HomeRunAPIError.missingConnection }
        return try await api.get("runners/\(id)/history", as: [JobHistoryEntry].self)
    }

    func rerunWorkflow(_ id: String, runUrl: String) async throws {
        guard let api else { throw HomeRunAPIError.missingConnection }
        try await api.command("runners/\(id)/rerun", method: "POST", body: RunUrlRequest(runUrl: runUrl))
    }

    func runStatus(_ id: String, runUrl: String) async throws -> RunStatusResponse {
        guard let api else { throw HomeRunAPIError.missingConnection }
        return try await api.post("runners/\(id)/run-status", body: RunUrlRequest(runUrl: runUrl), as: RunStatusResponse.self)
    }

    func clearHistory(_ id: String) async throws {
        guard let api else { throw HomeRunAPIError.missingConnection }
        try await api.command("runners/\(id)/history", method: "DELETE")
    }

    func deleteHistoryEntry(_ id: String, startedAt: String) async throws {
        guard let api else { throw HomeRunAPIError.missingConnection }
        try await api.command("runners/\(id)/history/entry", method: "DELETE", body: HistoryEntryRequest(startedAt: startedAt))
    }

    func updatePreferences(_ value: Preferences) async throws {
        guard let api else { throw HomeRunAPIError.missingConnection }
        preferences = try await api.put("preferences", body: value, as: Preferences.self)
    }

    func signInWithToken(_ token: String) async throws {
        guard let api else { throw HomeRunAPIError.missingConnection }
        authStatus = try await api.post("auth/token", body: TokenRequest(token: token), as: AuthStatus.self)
        await refresh()
    }

    func signOutFromGitHub() async throws {
        guard let api else { throw HomeRunAPIError.missingConnection }
        try await api.command("auth", method: "DELETE")
        authStatus = AuthStatus(authenticated: false, user: nil)
        repositories = []
    }

    func startDeviceFlow() async throws -> DeviceFlowResponse {
        guard let api else { throw HomeRunAPIError.missingConnection }
        return try await api.post("auth/device", as: DeviceFlowResponse.self)
    }

    func pollDeviceFlow(_ flow: DeviceFlowResponse) async throws -> AuthStatus {
        guard let api else { throw HomeRunAPIError.missingConnection }
        let value = try await api.post("auth/device/poll", body: DevicePollRequest(deviceCode: flow.deviceCode, interval: flow.interval), as: AuthStatus.self)
        authStatus = value
        await refresh()
        return value
    }

    func serviceAction(_ action: String) async throws {
        guard let api else { throw HomeRunAPIError.missingConnection }
        let _: JSONValue = try await api.post("service/\(action)", as: JSONValue.self)
        serviceStatus = try await api.get("service/status", as: ServiceStatusResponse.self)
    }

    func loadServiceStatus() async throws {
        guard let api else { throw HomeRunAPIError.missingConnection }
        serviceStatus = try await api.get("service/status", as: ServiceStatusResponse.self)
    }

    func checkForUpdates() async throws {
        guard let api else { throw HomeRunAPIError.missingConnection }
        updateStatus = try await api.get("updates/check", as: UpdateCheckResponse.self)
    }

    func shutdownDaemon() async throws {
        guard let api else { throw HomeRunAPIError.missingConnection }
        try await api.command("daemon/shutdown")
        forgetConnection()
    }
}
