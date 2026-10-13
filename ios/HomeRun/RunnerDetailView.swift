import SwiftUI

struct RunnerDetailView: View {
    @EnvironmentObject private var model: AppModel
    let runnerId: String

    @State private var section = "Overview"
    @State private var logs: [LogEntry] = []
    @State private var steps: StepsResponse?
    @State private var history: [JobHistoryEntry] = []
    @State private var selectedStep: Int?
    @State private var stepLogs: StepLogsResponse?
    @State private var errorMessage: String?
    @State private var confirmDelete = false
    @State private var confirmClearHistory = false
    @State private var working = false
    @State private var runStatuses: [String: String] = [:]

    private var runner: RunnerInfo? { model.runners.first(where: { $0.id == runnerId }) }
    private var cpu: RunnerMetrics? { model.metric(for: runnerId) }

    var body: some View {
        Group {
            if let runner {
                List {
                    Section {
                        VStack(alignment: .leading, spacing: 10) {
                            HStack {
                                VStack(alignment: .leading, spacing: 4) {
                                    Text(runner.config.name).font(.title2.bold())
                                    Text(runner.config.repository).foregroundStyle(.secondary)
                                }
                                Spacer()
                                RunnerStateBadge(state: runner.state)
                            }
                            Text("Mode: \(runner.config.mode.capitalized) · PID: \(runner.pid.map { String($0) } ?? "—")")
                                .font(.footnote)
                                .foregroundStyle(.secondary)
                            if let current = runner.currentJob, !current.isEmpty {
                                Label("Running: \(current)", systemImage: "bolt.fill")
                                    .font(.subheadline.weight(.medium))
                            }
                            if let context = runner.jobContext {
                                HStack(spacing: 8) {
                                    Label(context.branch, systemImage: "arrow.branch")
                                    if let pr = context.prNumber { Text("PR #\(pr)") }
                                }
                                .font(.footnote)
                            }
                            if let error = runner.errorMessage, !error.isEmpty {
                                Label(error, systemImage: "exclamationmark.triangle.fill")
                                    .font(.footnote)
                                    .foregroundStyle(.red)
                            }
                            HStack(spacing: 10) {
                                Button { Task { await action("start") } } label: {
                                    Label("Start", systemImage: "play.fill")
                                }
                                .buttonStyle(.bordered)
                                .disabled(working || runner.state == .online || runner.state == .busy)
                                Button { Task { await action("stop") } } label: {
                                    Label("Stop", systemImage: "stop.fill")
                                }
                                .buttonStyle(.bordered)
                                .disabled(working || (runner.state != .online && runner.state != .busy))
                                Button { Task { await action("restart") } } label: {
                                    Label("Restart", systemImage: "arrow.clockwise")
                                }
                                .buttonStyle(.bordered)
                                .disabled(working)
                            }
                            .labelStyle(.titleAndIcon)
                        }
                        .padding(.vertical, 4)
                    }

                    Section {
                        Picker("Details", selection: $section) {
                            ForEach(["Overview", "Logs", "Steps", "History"], id: \.self) { Text($0).tag($0) }
                        }
                        .pickerStyle(.segmented)
                        .listRowInsets(EdgeInsets(top: 8, leading: 0, bottom: 8, trailing: 0))
                    }

                    switch section {
                    case "Logs": logsSection
                    case "Steps": stepsSection
                    case "History": historySection
                    default: overviewSection(runner)
                    }
                }
                .navigationTitle("Runner")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarTrailing) {
                        Menu {
                            Button("Refresh details", systemImage: "arrow.clockwise") { Task { await loadSection() } }
                            Button("Delete runner", systemImage: "trash", role: .destructive) { confirmDelete = true }
                        } label: { Image(systemName: "ellipsis.circle") }
                    }
                }
                .confirmationDialog("Delete this runner?", isPresented: $confirmDelete, titleVisibility: .visible) {
                    Button("Delete runner", role: .destructive) {
                        Task {
                            do { try await model.deleteRunner(runnerId) }
                            catch { errorMessage = error.localizedDescription }
                        }
                    }
                } message: {
                    Text("The runner will be removed from the host and deregistered from GitHub.")
                }
                .confirmationDialog("Clear all job history?", isPresented: $confirmClearHistory, titleVisibility: .visible) {
                    Button("Clear history", role: .destructive) {
                        Task {
                            do {
                                try await model.clearHistory(runnerId)
                                history = []
                            } catch { errorMessage = error.localizedDescription }
                        }
                    }
                }
                .alert("Operation failed", isPresented: Binding(
                    get: { errorMessage != nil },
                    set: { if !$0 { errorMessage = nil } }
                )) {
                    Button("OK", role: .cancel) { errorMessage = nil }
                } message: { Text(errorMessage ?? "") }
                .task(id: section) { await loadSection() }
                .refreshable {
                    await model.refresh()
                    await loadSection()
                }
            } else {
                EmptyStateView(title: "Runner unavailable", message: "It may have been removed or the list has not refreshed.", symbol: "questionmark.circle")
                    .navigationTitle("Runner")
            }
        }
    }

    @ViewBuilder
    private func overviewSection(_ runner: RunnerInfo) -> some View {
        Section("Configuration") {
            LabeledContent("Repository", value: runner.config.repository)
            LabeledContent("Mode", value: runner.config.mode.capitalized)
            LabeledContent("Working directory", value: runner.config.workDir)
            LabeledContent("Uptime", value: formatDuration(runner.uptimeSecs))
            LabeledContent("Completed jobs", value: String(runner.jobsCompleted))
            LabeledContent("Failed jobs", value: String(runner.jobsFailed))
            LabeledContent("Container ID", value: runner.containerId ?? "—")
            if let groupId = runner.config.groupId {
                LabeledContent("Group", value: groupId)
            }
            ForEach(runner.config.labels, id: \.self) { label in
                Label(label, systemImage: "tag")
            }
        }
        Section("Resource usage") {
            LabeledContent("CPU", value: cpu.map { String(format: "%.1f%%", $0.cpuPercent) } ?? "—")
            LabeledContent("Memory", value: cpu.map { formatBytes($0.memoryBytes) } ?? "—")
            if let daemon = model.metrics?.daemon {
                LabeledContent("Host CPU", value: String(format: "%.1f%%", model.metrics?.system.cpuPercent ?? 0))
                LabeledContent("Daemon PID", value: String(daemon.pid))
            }
        }
        if let last = runner.lastCompletedJob {
            Section("Last completed job") {
                Label(last.jobName, systemImage: last.succeeded ? "checkmark.circle.fill" : "xmark.circle.fill")
                    .foregroundStyle(last.succeeded ? .green : .red)
                LabeledContent("Duration", value: formatDuration(last.durationSecs))
                LabeledContent("Completed", value: formatTimestamp(last.completedAt))
                if let branch = last.branch { LabeledContent("Branch", value: branch) }
                if let url = last.runUrl, let target = URL(string: url) {
                    Link("Open workflow run", destination: target)
                }
            }
        }
    }

    private var logsSection: some View {
        Section("Live runner logs") {
            if logs.isEmpty {
                Text("Waiting for log output…").foregroundStyle(.secondary)
            }
            ForEach(logs.suffix(500)) { entry in
                VStack(alignment: .leading, spacing: 3) {
                    HStack {
                        Text(formatTimestamp(entry.timestamp))
                        Spacer()
                        Text(entry.stream.uppercased())
                    }
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    Text(entry.line)
                        .font(.system(.caption, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .padding(.vertical, 2)
            }
        }
    }

    private var stepsSection: some View {
        Section("Job steps") {
            if let steps, steps.steps.isEmpty {
                Text("No step details are available for the current job.").foregroundStyle(.secondary)
            } else if steps == nil {
                ProgressView("Loading job steps…")
            }
            ForEach(steps?.steps ?? []) { step in
                VStack(alignment: .leading, spacing: 6) {
                    Button {
                        Task {
                            selectedStep = selectedStep == step.number ? nil : step.number
                            stepLogs = nil
                            if selectedStep != nil {
                                do { stepLogs = try await model.loadStepLogs(runnerId, step: step.number) }
                                catch { errorMessage = error.localizedDescription }
                            }
                        }
                    } label: {
                        HStack(alignment: .top) {
                            VStack(alignment: .leading, spacing: 4) {
                                Text("\(step.number). \(step.name)").foregroundStyle(.primary)
                                Text(step.status.capitalized).font(.caption).foregroundStyle(.secondary)
                            }
                            Spacer()
                            Image(systemName: selectedStep == step.number ? "chevron.up" : "chevron.down")
                                .font(.caption)
                        }
                    }
                    if selectedStep == step.number, let stepLogs {
                        Text(stepLogs.lines.joined(separator: "\n"))
                            .font(.system(.caption2, design: .monospaced))
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(8)
                            .background(.quaternary, in: RoundedRectangle(cornerRadius: 8))
                    }
                }
                .padding(.vertical, 4)
            }
        }
    }

    private var historySection: some View {
        Section {
            HStack {
                Text("Job history").font(.headline)
                Spacer()
                Button("Clear", role: .destructive) { confirmClearHistory = true }
                    .disabled(history.isEmpty)
            }
            if history.isEmpty {
                Text("No job history for this runner.").foregroundStyle(.secondary)
            }
            ForEach(history) { entry in
                VStack(alignment: .leading, spacing: 7) {
                    HStack(alignment: .top) {
                        Image(systemName: entry.succeeded ? "checkmark.circle.fill" : "xmark.circle.fill")
                            .foregroundStyle(entry.succeeded ? .green : .red)
                        VStack(alignment: .leading, spacing: 3) {
                            Text(entry.jobName).font(.headline)
                            Text("Job #\(entry.jobNumber) · \(formatTimestamp(entry.startedAt))")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        Spacer()
                        Text(entry.succeeded ? "Passed" : "Failed")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(entry.succeeded ? .green : .red)
                    }
                    if let branch = entry.branch {
                        Label(branch, systemImage: "arrow.branch").font(.caption).foregroundStyle(.secondary)
                    }
                    if let runUrl = entry.runUrl, let target = URL(string: runUrl) {
                        ViewThatFits(in: .horizontal) {
                            HStack {
                                Link("Open workflow", destination: target)
                                Button("Check status") {
                                    Task {
                                        do {
                                            let value = try await model.runStatus(runnerId, runUrl: runUrl)
                                            runStatuses[entry.id] = "\(value.status) · \(value.conclusion ?? "no conclusion")"
                                        } catch { errorMessage = error.localizedDescription }
                                    }
                                }
                                Button("Re-run") {
                                    Task {
                                        do {
                                            try await model.rerunWorkflow(runnerId, runUrl: runUrl)
                                            runStatuses[entry.id] = "Re-run requested"
                                        } catch { errorMessage = error.localizedDescription }
                                    }
                                }
                            }
                            HStack {
                                Link("Open workflow", destination: target)
                                Button("Status") {
                                    Task {
                                        do {
                                            let value = try await model.runStatus(runnerId, runUrl: runUrl)
                                            runStatuses[entry.id] = "\(value.status) · \(value.conclusion ?? "no conclusion")"
                                        } catch { errorMessage = error.localizedDescription }
                                    }
                                }
                                Button("Re-run") {
                                    Task {
                                        do {
                                            try await model.rerunWorkflow(runnerId, runUrl: runUrl)
                                            runStatuses[entry.id] = "Re-run requested"
                                        } catch { errorMessage = error.localizedDescription }
                                    }
                                }
                            }
                        }
                        .buttonStyle(.bordered)
                        if let status = runStatuses[entry.id] {
                            Text(status).font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    if let attempt = entry.latestAttempt {
                        Text("Latest attempt #\(attempt.attempt) · \(attempt.succeeded ? "passed" : "failed") · \(attempt.runnerName)")
                            .font(.caption)
                            .foregroundStyle(attempt.succeeded ? Color.green : Color.secondary)
                    }
                    if let error = entry.errorMessage, !error.isEmpty {
                        Text(error).font(.caption).foregroundStyle(.red)
                    }
                    ForEach(entry.steps) { step in
                        HStack(spacing: 8) {
                            Image(systemName: step.status == "succeeded" ? "checkmark.circle" : "circle")
                                .foregroundStyle(step.status == "succeeded" ? .green : .secondary)
                            Text(step.name).font(.caption)
                            Spacer()
                            Text(step.status).font(.caption2).foregroundStyle(.secondary)
                        }
                    }
                    Button("Delete history entry", role: .destructive) {
                        Task {
                            do {
                                try await model.deleteHistoryEntry(runnerId, startedAt: entry.startedAt)
                                history.removeAll { $0.id == entry.id }
                            } catch { errorMessage = error.localizedDescription }
                        }
                    }
                    .font(.caption)
                }
                .padding(.vertical, 6)
            }
        }
    }

    private func action(_ action: String) async {
        working = true
        defer { working = false }
        do { try await model.runnerAction(runnerId, action: action) }
        catch { errorMessage = error.localizedDescription }
    }

    private func loadSection() async {
        switch section {
        case "Logs":
            do {
                logs = try await model.loadRunnerLogs(runnerId)
            } catch {
                errorMessage = error.localizedDescription
            }
            guard let stream = model.logsStream(for: runnerId) else { return }
            do {
                for try await entry in stream {
                    guard !Task.isCancelled else { return }
                    logs.append(entry)
                    if logs.count > 1000 { logs.removeFirst(logs.count - 1000) }
                }
            } catch {
                // The recent-log endpoint remains available if streaming drops.
            }
        case "Steps":
            do { steps = try await model.loadSteps(runnerId) }
            catch { steps = nil }
        case "History":
            do { history = try await model.loadHistory(runnerId) }
            catch { errorMessage = error.localizedDescription }
        default:
            break
        }
    }
}
