import SwiftUI

struct DashboardView: View {
    @EnvironmentObject private var model: AppModel

    private var activeCount: Int {
        model.runners.filter { $0.state == .online || $0.state == .busy }.count
    }
    private var busyCount: Int { model.runners.filter { $0.state == .busy }.count }
    private var errorCount: Int { model.runners.filter { $0.state == .error }.count }
    private var columns: [GridItem] { [GridItem(.flexible()), GridItem(.flexible())] }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                HStack {
                    VStack(alignment: .leading, spacing: 3) {
                        Text("HomeRun").font(.largeTitle.bold())
                        Text(model.authenticated ? "GitHub · @\(model.signedInUser)" : "GitHub sign-in required")
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button {
                        Task { await model.refresh() }
                    } label: {
                        Image(systemName: "arrow.clockwise")
                            .font(.title3)
                            .padding(10)
                            .background(.thinMaterial, in: Circle())
                    }
                    .accessibilityLabel("Refresh dashboard")
                }

                LazyVGrid(columns: columns, spacing: 12) {
                    MetricTile(title: "Runners", value: "\(model.runners.count)", detail: "\(activeCount) active · \(errorCount) errors", symbol: "server.rack")
                    MetricTile(title: "Busy", value: "\(busyCount)", detail: "Executing jobs", symbol: "bolt.fill")
                    MetricTile(title: "CPU", value: String(format: "%.1f%%", model.metrics?.system.cpuPercent ?? 0), detail: "Host utilization", symbol: "cpu")
                    MetricTile(title: "Memory", value: memorySummary, detail: memoryDetail, symbol: "memorychip")
                    MetricTile(title: "Disk", value: diskSummary, detail: diskDetail, symbol: "internaldrive")
                }

                if let daemon = model.metrics?.daemon {
                    VStack(alignment: .leading, spacing: 12) {
                        Label("Daemon", systemImage: "gearshape.2")
                            .font(.headline)
                        HStack {
                            Label("PID \(daemon.pid)", systemImage: "number")
                            Spacer()
                            Label(formatDuration(daemon.uptimeSeconds), systemImage: "clock")
                        }
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        HStack {
                            Text("CPU")
                            ProgressView(value: min(100, max(0, daemon.cpuPercent)), total: 100)
                            Text("\(daemon.cpuPercent, specifier: "%.1f")%")
                                .monospacedDigit()
                                .font(.caption)
                        }
                        HStack {
                            Text("Daemon memory")
                            Spacer()
                            Text(formatBytes(daemon.memoryBytes)).monospacedDigit()
                        }
                        .font(.subheadline)
                    }
                    .padding()
                    .background(.thinMaterial, in: RoundedRectangle(cornerRadius: 16))
                }

                VStack(alignment: .leading, spacing: 10) {
                    HStack {
                        Text("Runner activity").font(.title3.bold())
                        Spacer()
                        NavigationLink("View all") { RunnersView() }
                            .font(.subheadline)
                    }
                    if model.runners.isEmpty {
                        EmptyStateView(title: "No runners yet", message: "Create a runner from a GitHub repository to get started.", symbol: "server.rack")
                    } else {
                        ForEach(model.runners.sorted(by: { $0.config.name.localizedCaseInsensitiveCompare($1.config.name) == .orderedAscending })) { runner in
                            NavigationLink {
                                RunnerDetailView(runnerId: runner.id)
                            } label: {
                                RunnerSummaryRow(runner: runner, cpu: model.metric(for: runner.id)?.cpuPercent)
                            }
                            .buttonStyle(.plain)
                            Divider()
                        }
                    }
                }

                if let error = model.lastError {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .font(.footnote)
                        .foregroundStyle(.red)
                }
                Text("Daemon v\(model.daemonVersion.isEmpty ? "—" : model.daemonVersion) · Refreshes every 2 seconds")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
            .padding()
        }
        .navigationBarHidden(true)
        .refreshable { await model.refresh() }
        .task { await model.refresh() }
    }

    private var memorySummary: String {
        guard let system = model.metrics?.system else { return "—" }
        return formatBytes(system.memoryUsedBytes)
    }

    private var memoryDetail: String {
        guard let system = model.metrics?.system else { return "Host memory" }
        return "of \(formatBytes(system.memoryTotalBytes))"
    }

    private var diskSummary: String {
        guard let system = model.metrics?.system else { return "—" }
        return formatBytes(system.diskUsedBytes)
    }

    private var diskDetail: String {
        guard let system = model.metrics?.system else { return "Host disk" }
        return "of \(formatBytes(system.diskTotalBytes))"
    }
}
