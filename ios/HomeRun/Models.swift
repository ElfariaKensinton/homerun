import Foundation

struct HealthResponse: Decodable {
    let status: String
    let version: String?
    let pid: Int?
}

struct GitHubUser: Codable {
    var login: String
    var avatarUrl: String
}

struct AuthStatus: Codable {
    var authenticated: Bool
    var user: GitHubUser?
}

struct DeviceFlowResponse: Codable, Identifiable {
    var deviceCode: String
    var userCode: String
    var verificationUri: String
    var expiresIn: Int
    var interval: Int
    var id: String { deviceCode }
}

enum RunnerState: String, Codable, CaseIterable {
    case creating, registering, online, busy, stopping, offline, error, deleting

    var title: String {
        switch self {
        case .creating: "Creating"
        case .registering: "Registering"
        case .online: "Online"
        case .busy: "Busy"
        case .stopping: "Stopping"
        case .offline: "Offline"
        case .error: "Error"
        case .deleting: "Deleting"
        }
    }

    var symbol: String {
        switch self {
        case .online: "checkmark.circle.fill"
        case .busy: "bolt.fill"
        case .error: "exclamationmark.triangle.fill"
        case .offline: "pause.circle.fill"
        case .creating, .registering, .stopping, .deleting: "arrow.triangle.2.circlepath"
        }
    }
}

struct ContainerConfig: Codable {
    var image: String
    // The daemon's JSON shape for environment entries is [["KEY", "value"], ...].
    var extraEnv: [[String]]
}

struct RunnerConfig: Codable {
    var id: String
    var name: String
    var repoOwner: String
    var repoName: String
    var labels: [String]
    var mode: String
    var workDir: String
    var groupId: String?
    var container: ContainerConfig?

    var repository: String { "\(repoOwner)/\(repoName)" }
}

struct JobContext: Codable {
    var branch: String
    var prNumber: Int?
    var prUrl: String?
    var runUrl: String
    var jobId: Int?
}

struct RunAttempt: Codable {
    var attempt: Int
    var succeeded: Bool
    var runnerName: String
    var completedAt: String
    var runUrl: String?
}

struct CompletedJob: Codable {
    var jobName: String
    var succeeded: Bool
    var completedAt: String
    var durationSecs: Int
    var branch: String?
    var prNumber: Int?
    var runUrl: String?
    var errorMessage: String?
    var latestAttempt: RunAttempt?
}

struct RunnerInfo: Codable, Identifiable {
    var config: RunnerConfig
    var state: RunnerState
    var pid: Int?
    var containerId: String?
    var uptimeSecs: Int?
    var jobsCompleted: Int
    var jobsFailed: Int
    var currentJob: String?
    var jobContext: JobContext?
    var errorMessage: String?
    var jobStartedAt: String?
    var lastCompletedJob: CompletedJob?
    var estimatedJobDurationSecs: Int?
    var id: String { config.id }
}

struct SystemMetrics: Codable {
    var cpuPercent: Double
    var memoryUsedBytes: UInt64
    var memoryTotalBytes: UInt64
    var diskUsedBytes: UInt64
    var diskTotalBytes: UInt64
}

struct RunnerMetrics: Codable, Identifiable {
    var runnerId: String
    var cpuPercent: Double
    var memoryBytes: UInt64
    var id: String { runnerId }
}

struct ChildProcessInfo: Codable, Identifiable {
    var pid: Int
    var runnerId: String
    var runnerName: String
    var cpuPercent: Double
    var memoryBytes: UInt64
    var id: Int { pid }
}

struct DaemonMetrics: Codable {
    var pid: Int
    var uptimeSeconds: Int
    var cpuPercent: Double
    var memoryBytes: UInt64
    var childProcesses: [ChildProcessInfo]
}

struct MetricsResponse: Codable {
    var system: SystemMetrics
    var runners: [RunnerMetrics]
    var daemon: DaemonMetrics?
}

struct RepoInfo: Codable, Identifiable {
    var id: Int
    var fullName: String
    var name: String
    var owner: String
    var isPrivate: Bool
    var htmlUrl: String
    var isOrg: Bool
}

struct DiscoveredRepo: Codable, Identifiable {
    var fullName: String
    var source: String
    var workflowFiles: [String]
    var localPath: String?
    var matchedLabels: [String]
    var id: String { "\(source):\(fullName)" }
}

struct ScanResults: Codable {
    var lastScanAt: String
    var localResults: [DiscoveredRepo]
    var remoteResults: [DiscoveredRepo]
    var mergedResults: [DiscoveredRepo]
}

struct LogEntry: Codable, Identifiable {
    var runnerId: String
    var timestamp: String
    var line: String
    var stream: String
    var id: String { "\(timestamp)-\(stream)-\(line.hashValue)" }
}

struct DaemonLogEntry: Codable, Identifiable {
    var timestamp: String
    var level: String
    var target: String
    var message: String
    var id: String { "\(timestamp)-\(target)-\(message.hashValue)" }
}

struct StepInfo: Codable, Identifiable {
    var number: Int
    var name: String
    var status: String
    var startedAt: String?
    var completedAt: String?
    var id: Int { number }
}

struct StepsResponse: Codable {
    var jobName: String
    var steps: [StepInfo]
    var stepsDiscovered: Int
}

struct StepLogsResponse: Codable {
    var stepNumber: Int
    var stepName: String
    var lines: [String]
}

struct JobHistoryEntry: Codable, Identifiable {
    var jobName: String
    var startedAt: String
    var completedAt: String
    var succeeded: Bool
    var branch: String?
    var prNumber: Int?
    var runUrl: String?
    var errorMessage: String?
    var steps: [StepInfo]
    var latestAttempt: RunAttempt?
    var jobNumber: Int
    var id: String { "\(jobNumber)-\(startedAt)" }
}

struct RunStatusResponse: Codable {
    var status: String
    var conclusion: String?
}

struct ContainerEnvironmentLine {
    static func parse(_ text: String) -> [[String]] {
        text.split(whereSeparator: \.isNewline).compactMap { line in
            let parts = line.split(separator: "=", maxSplits: 1, omittingEmptySubsequences: false)
            guard parts.count == 2 else { return nil }
            let key = parts[0].trimmingCharacters(in: .whitespaces)
            guard !key.isEmpty else { return nil }
            return [key, String(parts[1])]
        }
    }

    static func format(_ values: [[String]]) -> String {
        values.compactMap { pair in
            guard pair.count >= 2 else { return nil }
            return "\(pair[0])=\(pair[1])"
        }.joined(separator: "\n")
    }
}

struct CreateRunnerRequest: Encodable {
    var repoFullName: String
    var name: String?
    var labels: [String]?
    var mode: String?
    var container: ContainerConfig?
}

struct CreateBatchRequest: Encodable {
    var repoFullName: String
    var count: Int
    var labels: [String]?
    var mode: String?
    var container: ContainerConfig?
}

struct BatchCreateResponse: Decodable {
    struct Failure: Decodable, Identifiable {
        var index: Int
        var error: String
        var id: Int { index }
    }
    var groupId: String
    var runners: [RunnerInfo]
    var errors: [Failure]
}

struct GroupActionResponse: Decodable {
    struct Result: Decodable, Identifiable {
        var runnerId: String
        var success: Bool
        var error: String?
        var id: String { runnerId }
    }
    var groupId: String
    var results: [Result]
}

struct ScaleGroupResponse: Decodable {
    var groupId: String
    var previousCount: Int
    var targetCount: Int
    var actualCount: Int
    var added: [RunnerInfo]
    var removed: [String]
    var skippedBusy: [String]
}

struct Preferences: Codable {
    var startRunnersOnLaunch = false
    var notifyStatusChanges = true
    var notifyJobCompletions = true
    var scanLabels: [String] = ["self-hosted"]
    var workspacePath: String?
    var autoScan = false
    var watchNewRepositories = false
    var autoCreateRunnerForNewRepositories = false
    var customRunnerDownloadUrl: String?
    var artifactResultsUrl: String?
}

struct DockerStatusResponse: Codable {
    var available: Bool
    var error: String?
}

struct ServiceStatusResponse: Decodable {
    var installed: Bool
}

struct UpdateCheckResponse: Decodable {
    var updateAvailable: Bool
    var current: String?
    var latest: String?
}

struct RunnerEvent: Decodable {
    var runnerId: String
    var eventType: String
    var data: JSONValue?
    var timestamp: String
}

enum JSONValue: Decodable {
    case string(String), number(Double), bool(Bool), object([String: JSONValue]), array([JSONValue]), null

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() { self = .null }
        else if let value = try? container.decode(Bool.self) { self = .bool(value) }
        else if let value = try? container.decode(Double.self) { self = .number(value) }
        else if let value = try? container.decode(String.self) { self = .string(value) }
        else if let value = try? container.decode([String: JSONValue].self) { self = .object(value) }
        else if let value = try? container.decode([JSONValue].self) { self = .array(value) }
        else { throw DecodingError.dataCorruptedError(in: container, debugDescription: "Unsupported JSON value") }
    }
}

struct ScanLocalRequest: Encodable {
    var path: String
    var labels: [String]?
}

struct RunUrlRequest: Encodable {
    var runUrl: String
}

struct HistoryEntryRequest: Encodable {
    var startedAt: String
}

struct ScaleRequest: Encodable {
    var count: Int
}

struct TokenRequest: Encodable {
    var token: String
}

struct DevicePollRequest: Encodable {
    var deviceCode: String
    var interval: Int
}

struct EmptyPayload: Encodable {}
