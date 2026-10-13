import SwiftUI

struct RunnerStateBadge: View {
    let state: RunnerState

    private var color: Color {
        switch state {
        case .online: return .green
        case .busy: return .orange
        case .error: return .red
        case .offline: return .secondary
        case .creating, .registering, .stopping, .deleting: return .indigo
        }
    }

    var body: some View {
        Label(state.title, systemImage: state.symbol)
            .font(.caption.weight(.semibold))
            .foregroundStyle(color)
            .padding(.horizontal, 9)
            .padding(.vertical, 5)
            .background(color.opacity(0.12), in: Capsule())
    }
}

struct MetricTile: View {
    let title: String
    let value: String
    let detail: String
    let symbol: String

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label(title, systemImage: symbol)
                .font(.subheadline)
                .foregroundStyle(.secondary)
            Text(value)
                .font(.title2.bold())
                .lineLimit(1)
                .minimumScaleFactor(0.7)
            Text(detail)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(2)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding()
        .background(.thinMaterial, in: RoundedRectangle(cornerRadius: 16))
    }
}

struct RunnerSummaryRow: View {
    let runner: RunnerInfo
    var cpu: Double? = nil

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: runner.config.mode == "container" ? "shippingbox" : "server.rack")
                .font(.title3)
                .foregroundStyle(.indigo)
                .frame(width: 36, height: 36)
                .background(.indigo.opacity(0.1), in: RoundedRectangle(cornerRadius: 10))
            VStack(alignment: .leading, spacing: 4) {
                Text(runner.config.name)
                    .font(.headline)
                    .lineLimit(1)
                Text(runner.config.repository)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                HStack(spacing: 8) {
                    Text("\(runner.jobsCompleted) jobs")
                    if runner.jobsFailed > 0 { Text("· \(runner.jobsFailed) failed").foregroundStyle(.red) }
                    if let cpu { Text("· CPU \(cpu, specifier: "%.1f")%") }
                }
                .font(.caption2)
                .foregroundStyle(.secondary)
            }
            Spacer(minLength: 4)
            RunnerStateBadge(state: runner.state)
        }
        .padding(.vertical, 4)
    }
}

struct EmptyStateView: View {
    let title: String
    let message: String
    let symbol: String

    var body: some View {
        ContentUnavailableView(title, systemImage: symbol, description: Text(message))
            .padding(.vertical, 12)
    }
}

struct RunnerGroupSummary: Identifiable {
    var id: String
    var runners: [RunnerInfo]
}

func formatBytes(_ bytes: UInt64) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(clamping: bytes), countStyle: .memory)
}

func formatDuration(_ seconds: Int?) -> String {
    guard let seconds else { return "—" }
    let value = max(0, seconds)
    if value >= 86_400 { return "\(value / 86_400)d \((value % 86_400) / 3_600)h" }
    if value >= 3_600 { return "\(value / 3_600)h \((value % 3_600) / 60)m" }
    if value >= 60 { return "\(value / 60)m \(value % 60)s" }
    return "\(value)s"
}

func formatTimestamp(_ value: String?) -> String {
    guard let value, !value.isEmpty else { return "—" }
    let parser = ISO8601DateFormatter()
    if let date = parser.date(from: value) {
        return date.formatted(date: .abbreviated, time: .standard)
    }
    parser.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    if let date = parser.date(from: value) {
        return date.formatted(date: .abbreviated, time: .standard)
    }
    return value
}
