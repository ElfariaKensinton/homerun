import Foundation

enum HomeRunAPIError: LocalizedError {
    case invalidEndpoint
    case insecureEndpoint
    case missingConnection
    case http(Int, String)
    case invalidResponse

    var errorDescription: String? {
        switch self {
        case .invalidEndpoint:
            return "Enter a valid HTTPS endpoint, including https://."
        case .insecureEndpoint:
            return "Use HTTPS for remote connections. HTTP is allowed only for localhost or a private LAN host."
        case .missingConnection:
            return "Connect to a HomeRun daemon first."
        case let .http(code, message):
            return message.isEmpty ? "The HomeRun daemon returned HTTP \(code)." : "HTTP \(code): \(message)"
        case .invalidResponse:
            return "The daemon returned an unreadable response."
        }
    }
}

private func isTrustedLocalNetworkHost(_ host: String) -> Bool {
    let normalized = host.lowercased()
    if normalized == "localhost" || normalized == "::1" || normalized.hasSuffix(".local") {
        return true
    }

    let pieces = normalized.split(separator: ".")
    let octets = pieces.compactMap { Int($0) }
    guard pieces.count == 4,
          octets.count == 4,
          octets.allSatisfy({ (0...255).contains($0) }) else {
        return false
    }

    let first = octets[0]
    let second = octets[1]
    return first == 10
        || first == 127
        || (first == 169 && second == 254)
        || (first == 172 && (16...31).contains(second))
        || (first == 192 && second == 168)
}

private struct AnyEncodable: Encodable {
    private let encodeValue: (Encoder) throws -> Void
    init<T: Encodable>(_ value: T) { encodeValue = value.encode }
    func encode(to encoder: Encoder) throws { try encodeValue(encoder) }
}

final class HomeRunAPI {
    let baseUrl: URL
    private let token: String
    private let session: URLSession
    private let decoder: JSONDecoder
    private let encoder: JSONEncoder

    init(endpoint: String, token: String) throws {
        let value = endpoint.trimmingCharacters(in: .whitespacesAndNewlines)
        guard var components = URLComponents(string: value),
              let scheme = components.scheme?.lowercased(),
              let host = components.host,
              !host.isEmpty
        else { throw HomeRunAPIError.invalidEndpoint }

        guard scheme == "https" || (scheme == "http" && isTrustedLocalNetworkHost(host)) else {
            throw HomeRunAPIError.insecureEndpoint
        }
        guard components.user == nil, components.password == nil else {
            throw HomeRunAPIError.invalidEndpoint
        }
        components.path = components.path.hasSuffix("/") ? components.path : components.path + "/"
        components.query = nil
        components.fragment = nil
        guard let url = components.url else { throw HomeRunAPIError.invalidEndpoint }

        self.baseUrl = url
        self.token = token.trimmingCharacters(in: .whitespacesAndNewlines)
        self.session = URLSession(configuration: .default)
        self.decoder = JSONDecoder()
        self.decoder.keyDecodingStrategy = .convertFromSnakeCase
        self.encoder = JSONEncoder()
        self.encoder.keyEncodingStrategy = .convertToSnakeCase
    }

    private func makeRequest(path: String, method: String, body: Data? = nil, query: [URLQueryItem] = []) throws -> URLRequest {
        guard let url = URL(string: path, relativeTo: baseUrl)?.absoluteURL else {
            throw HomeRunAPIError.invalidEndpoint
        }
        var components = URLComponents(url: url, resolvingAgainstBaseURL: true)
        if !query.isEmpty { components?.queryItems = query }
        guard let finalUrl = components?.url else { throw HomeRunAPIError.invalidEndpoint }
        var request = URLRequest(url: finalUrl)
        request.httpMethod = method
        request.timeoutInterval = 30
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let body {
            request.httpBody = body
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        return request
    }

    private func perform(path: String, method: String = "GET", body: Data? = nil, query: [URLQueryItem] = []) async throws -> Data {
        let request = try makeRequest(path: path, method: method, body: body, query: query)
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else { throw HomeRunAPIError.invalidResponse }
        guard (200...299).contains(http.statusCode) else {
            throw HomeRunAPIError.http(http.statusCode, String(data: data, encoding: .utf8) ?? "")
        }
        return data
    }

    func get<T: Decodable>(_ path: String, query: [URLQueryItem] = [], as type: T.Type = T.self) async throws -> T {
        let data = try await perform(path: path, query: query)
        do { return try decoder.decode(type, from: data) }
        catch { throw HomeRunAPIError.invalidResponse }
    }

    func post<T: Decodable>(_ path: String, as type: T.Type = T.self) async throws -> T {
        let data = try await perform(path: path, method: "POST")
        do { return try decoder.decode(type, from: data) }
        catch { throw HomeRunAPIError.invalidResponse }
    }

    func post<B: Encodable, T: Decodable>(_ path: String, body: B, as type: T.Type = T.self) async throws -> T {
        let data = try encoder.encode(AnyEncodable(body))
        let result = try await perform(path: path, method: "POST", body: data)
        do { return try decoder.decode(type, from: result) }
        catch { throw HomeRunAPIError.invalidResponse }
    }

    func put<B: Encodable, T: Decodable>(_ path: String, body: B, as type: T.Type = T.self) async throws -> T {
        let data = try encoder.encode(AnyEncodable(body))
        let result = try await perform(path: path, method: "PUT", body: data)
        do { return try decoder.decode(type, from: result) }
        catch { throw HomeRunAPIError.invalidResponse }
    }

    func patch<B: Encodable, T: Decodable>(_ path: String, body: B, as type: T.Type = T.self) async throws -> T {
        let data = try encoder.encode(AnyEncodable(body))
        let result = try await perform(path: path, method: "PATCH", body: data)
        do { return try decoder.decode(type, from: result) }
        catch { throw HomeRunAPIError.invalidResponse }
    }

    func command(_ path: String, method: String = "POST") async throws {
        _ = try await perform(path: path, method: method)
    }

    func command<B: Encodable>(_ path: String, method: String, body: B) async throws {
        let data = try encoder.encode(AnyEncodable(body))
        _ = try await perform(path: path, method: method, body: data)
    }

    func health() async throws -> HealthResponse {
        try await get("health", as: HealthResponse.self)
    }

    func streamScan<B: Encodable>(path: String, scanId: String, body: B) -> AsyncThrowingStream<ScanProgressEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    let payload = try encoder.encode(AnyEncodable(body))
                    let request = try makeRequest(
                        path: path,
                        method: "POST",
                        body: payload,
                        query: [URLQueryItem(name: "scan_id", value: scanId)]
                    )
                    let (bytes, response) = try await session.bytes(for: request)
                    guard let http = response as? HTTPURLResponse, (200...299).contains(http.statusCode) else {
                        throw HomeRunAPIError.invalidResponse
                    }
                    for try await line in bytes.lines {
                        guard line.hasPrefix("data:") else { continue }
                        let payloadLine = line.dropFirst(5).trimmingCharacters(in: .whitespaces)
                        guard let data = payloadLine.data(using: .utf8) else { continue }
                        if let event = try? decoder.decode(ScanProgressEvent.self, from: data) {
                            continuation.yield(event)
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { @Sendable _ in task.cancel() }
        }
    }

    func events() -> AsyncThrowingStream<RunnerEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    guard var components = URLComponents(url: baseUrl, resolvingAgainstBaseURL: false) else {
                        throw HomeRunAPIError.invalidEndpoint
                    }
                    components.scheme = components.scheme == "https" ? "wss" : "ws"
                    components.path = components.path.hasSuffix("/") ? components.path + "events" : components.path + "/events"
                    guard let url = components.url else { throw HomeRunAPIError.invalidEndpoint }
                    var request = URLRequest(url: url)
                    request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
                    let socket = session.webSocketTask(with: request)
                    socket.resume()
                    defer { socket.cancel(with: .normalClosure, reason: nil) }
                    while !Task.isCancelled {
                        let message = try await socket.receive()
                        let data: Data
                        switch message {
                        case .data(let value): data = value
                        case .string(let value): data = Data(value.utf8)
                        @unknown default: continue
                        }
                        if let event = try? decoder.decode(RunnerEvent.self, from: data) {
                            continuation.yield(event)
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { @Sendable _ in task.cancel() }
        }
    }

    func streamDaemonLogs(level: String? = nil) -> AsyncThrowingStream<DaemonLogEntry, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var query: [URLQueryItem] = []
                    if let level, level != "All" {
                        query.append(URLQueryItem(name: "level", value: level.lowercased()))
                    }
                    let request = try makeRequest(path: "daemon/logs", method: "GET", query: query)
                    let (bytes, response) = try await session.bytes(for: request)
                    guard let http = response as? HTTPURLResponse, (200...299).contains(http.statusCode) else {
                        throw HomeRunAPIError.invalidResponse
                    }
                    for try await line in bytes.lines {
                        guard line.hasPrefix("data:") else { continue }
                        let payload = line.dropFirst(5).trimmingCharacters(in: .whitespaces)
                        guard let data = payload.data(using: .utf8) else { continue }
                        if let log = try? decoder.decode(DaemonLogEntry.self, from: data) {
                            continuation.yield(log)
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { @Sendable _ in task.cancel() }
        }
    }

    func streamLogs(runnerId: String) -> AsyncThrowingStream<LogEntry, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    let (bytes, response) = try await session.bytes(for: makeRequest(
                        path: "runners/\(runnerId)/logs",
                        method: "GET"
                    ))
                    guard let http = response as? HTTPURLResponse, (200...299).contains(http.statusCode) else {
                        throw HomeRunAPIError.invalidResponse
                    }
                    for try await line in bytes.lines {
                        guard line.hasPrefix("data:") else { continue }
                        let payload = line.dropFirst(5).trimmingCharacters(in: .whitespaces)
                        guard let data = payload.data(using: .utf8) else { continue }
                        if let log = try? decoder.decode(LogEntry.self, from: data) {
                            continuation.yield(log)
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { @Sendable _ in task.cancel() }
        }
    }
}
