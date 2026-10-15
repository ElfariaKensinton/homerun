import SwiftUI

struct RunnersView: View {
    @EnvironmentObject private var model: AppModel
    @State private var showingCreate = false
    @State private var errorMessage: String?
    @State private var pendingDeleteGroup: String?
    @State private var showingDeleteGroup = false

    private var groups: [RunnerGroupSummary] {
        let grouped = Dictionary(grouping: model.runners.compactMap { runner -> (String, RunnerInfo)? in
            guard let groupId = runner.config.groupId else { return nil }
            return (groupId, runner)
        }, by: { $0.0 })
        return grouped.map { RunnerGroupSummary(id: $0.key, runners: $0.value.map { $0.1 }) }
            .sorted { $0.id.localizedCaseInsensitiveCompare($1.id) == .orderedAscending }
    }

    var body: some View {
        List {
            if !groups.isEmpty {
                Section("Runner groups") {
                    ForEach(groups) { group in
                        RunnerGroupCard(
                            group: group,
                            perform: { action in await performGroup(group.id, action: action) },
                            scale: { count in
                                do {
                                    let result = try await model.scaleGroup(group.id, count: count)
                                    if !result.skippedBusy.isEmpty {
                                        errorMessage = "Scaled to \(result.actualCount)/\(result.targetCount). Busy runners were skipped: " + result.skippedBusy.joined(separator: ", ")
                                    } else if result.actualCount != result.targetCount {
                                        errorMessage = "Target was \(result.targetCount), but \(result.actualCount) runners are present."
                                    }
                                } catch { errorMessage = error.localizedDescription }
                            },
                            delete: {
                                pendingDeleteGroup = group.id
                                showingDeleteGroup = true
                            }
                        )
                    }
                }
            }

            Section("All runners · \(model.runners.count)") {
                if model.runners.isEmpty {
                    EmptyStateView(title: "No runners", message: "Create a runner or a batch of runners.", symbol: "server.rack")
                        .listRowSeparator(.hidden)
                } else {
                    ForEach(model.runners) { runner in
                        NavigationLink {
                            RunnerDetailView(runnerId: runner.id)
                        } label: {
                            RunnerSummaryRow(runner: runner, cpu: model.metric(for: runner.id)?.cpuPercent)
                        }
                    }
                }
            }
        }
        .navigationTitle("Runners")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button { showingCreate = true } label: { Image(systemName: "plus") }
                    .accessibilityLabel("Create runners")
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button { Task { await model.refresh() } } label: { Image(systemName: "arrow.clockwise") }
                    .accessibilityLabel("Refresh runners")
            }
        }
        .sheet(isPresented: $showingCreate) { CreateRunnerSheet() }
        .confirmationDialog(
            "Delete group and its runners?",
            isPresented: $showingDeleteGroup,
            titleVisibility: .visible
        ) {
            Button("Delete group", role: .destructive) {
                guard let id = pendingDeleteGroup else { return }
                Task {
                    do { _ = try await model.deleteGroup(id) }
                    catch { errorMessage = error.localizedDescription }
                }
            }
        } message: {
            Text("This removes all runners in the selected group.")
        }
        .alert("Runner operation", isPresented: Binding(
            get: { errorMessage != nil },
            set: { if !$0 { errorMessage = nil } }
        )) {
            Button("OK", role: .cancel) { errorMessage = nil }
        } message: { Text(errorMessage ?? "") }
        .refreshable { await model.refresh() }
        .task { await model.refresh() }
    }

    private func performGroup(_ groupId: String, action: String) async {
        do {
            let result = try await model.groupAction(groupId, action: action)
            let failures = result.results.filter { !$0.success }
            if !failures.isEmpty {
                errorMessage = failures.map { "\($0.runnerId): \($0.error ?? "operation failed")" }.joined(separator: "\n")
            }
        } catch { errorMessage = error.localizedDescription }
    }
}

private struct RunnerGroupCard: View {
    @EnvironmentObject private var model: AppModel
    let group: RunnerGroupSummary
    let perform: (String) async -> Void
    let scale: (Int) async -> Void
    let delete: () -> Void
    @State private var targetCount: Int

    init(group: RunnerGroupSummary, perform: @escaping (String) async -> Void, scale: @escaping (Int) async -> Void, delete: @escaping () -> Void) {
        self.group = group
        self.perform = perform
        self.scale = scale
        self.delete = delete
        _targetCount = State(initialValue: group.runners.count)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                VStack(alignment: .leading, spacing: 3) {
                    Text("Runner group · \(group.runners.first?.config.repository ?? "Unknown repository")")
                        .font(.headline)
                    Text(group.id)
                        .font(.caption2.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                Spacer()
                Text("\(group.runners.count) runners")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.secondary)
            }

            HStack(spacing: 8) {
                Button("Start") { Task { await perform("start") } }
                    .buttonStyle(.bordered)
                Button("Stop") { Task { await perform("stop") } }
                    .buttonStyle(.bordered)
                Button("Restart") { Task { await perform("restart") } }
                    .buttonStyle(.bordered)
                Spacer(minLength: 0)
                Menu {
                    Button("Delete group", role: .destructive, action: delete)
                } label: {
                    Image(systemName: "ellipsis")
                        .frame(width: 32, height: 32)
                }
            }
            Stepper("Target count: \(targetCount)", value: $targetCount, in: 0...20)
                .font(.subheadline)
            Button {
                Task { await scale(targetCount) }
            } label: {
                Label("Apply scale", systemImage: "arrow.left.and.right")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
        }
        .padding(.vertical, 6)
    }
}

struct CreateRunnerSheet: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var repository = ""
    @State private var scope = "repository"
    @State private var organization = ""
    @State private var manualRegistrationToken = ""
    @State private var runnerName = ""
    @State private var labels = "self-hosted"
    @State private var mode = "app"
    @State private var containerImage = "ubuntu:latest"
    @State private var environmentText = ""
    @State private var count = 1
    @State private var creating = false
    @State private var errorMessage: String?
    @State private var resultMessage: String?

    private var normalizedOrganization: String {
        organization.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var targetLabel: String {
        scope == "organization" ? normalizedOrganization : repository
    }

    private var suggestedRunnerName: String {
        let name = targetLabel.split(separator: "/").last.map(String.init) ?? "runner"
        return "\(name)-runner-1"
    }

    private var normalizedManualRegistrationToken: String {
        manualRegistrationToken.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var targetIsValid: Bool {
        if scope == "organization" {
            return !normalizedOrganization.isEmpty
                && !normalizedOrganization.contains("/")
                && !normalizedManualRegistrationToken.isEmpty
        }
        return !repository.isEmpty
    }

    var body: some View {
        NavigationStack {
            Form {
                Section("Runner target") {
                    Picker("Scope", selection: $scope) {
                        Text("Repository").tag("repository")
                        Text("Organization").tag("organization")
                    }
                    .pickerStyle(.segmented)
                    .onChange(of: scope) { _, newScope in
                        if newScope == "organization" {
                            count = 1
                            if mode == "container" { mode = "app" }
                        } else {
                            manualRegistrationToken = ""
                        }
                    }

                    if scope == "organization" {
                        TextField("Organization login", text: $organization)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                        Text("This runner can accept jobs from repositories across the organization.")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                        SecureField("GitHub registration token", text: $manualRegistrationToken)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                        Text("Copy the token from GitHub Organization Settings → Actions → Runners → New self-hosted runner. It is used once for initial setup and is not saved by HomeRun.")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    } else if model.repositories.isEmpty {
                        if let repositoryLoadError = model.repositoryLoadError {
                            VStack(alignment: .leading, spacing: 8) {
                                Label("Could not load GitHub repositories", systemImage: "exclamationmark.triangle")
                                    .foregroundStyle(.red)
                                Text(repositoryLoadError)
                                    .font(.footnote)
                                    .foregroundStyle(.secondary)
                                Button("Retry") {
                                    Task {
                                        await model.refresh()
                                        if repository.isEmpty { repository = model.repositories.first?.fullName ?? "" }
                                    }
                                }
                            }
                        } else if !model.authenticated {
                            Text("Sign in to GitHub in Settings to load repositories.")
                                .font(.footnote)
                                .foregroundStyle(.secondary)
                        } else if model.isRefreshing {
                            ProgressView("Loading GitHub repositories…")
                        } else {
                            Text("GitHub is connected, but no repositories were returned. Tap reload to try again.")
                                .font(.footnote)
                                .foregroundStyle(.secondary)
                        }
                    } else {
                        Picker("Repository", selection: $repository) {
                            ForEach(model.repositories) { repo in
                                Text(repo.fullName).tag(repo.fullName)
                            }
                        }
                    }
                    if count == 1 {
                        TextField("Custom runner name (optional)", text: $runnerName)
                        Text("Leave blank to generate a unique name, such as \(suggestedRunnerName).")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    } else {
                        Text("Names are generated automatically for all \(count) runners (for example, \(suggestedRunnerName)).")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                    TextField("Labels, comma-separated", text: $labels)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }

                Section("Runner configuration") {
                    Picker("Mode", selection: $mode) {
                        Text("App-managed").tag("app")
                        Text("Background service").tag("service")
                        if scope != "organization" {
                            Text("Docker container").tag("container")
                        }
                    }
                    .pickerStyle(.segmented)
                    if mode == "container" {
                        TextField("Container image", text: $containerImage)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                        TextField("Environment (KEY=value per line)", text: $environmentText, axis: .vertical)
                            .lineLimit(2...6)
                            .font(.system(.footnote, design: .monospaced))
                        if model.dockerStatus?.available == false {
                            Label(model.dockerStatus?.error ?? "Docker is not available on the host.", systemImage: "exclamationmark.triangle")
                                .font(.footnote)
                                .foregroundStyle(.orange)
                        } else if model.dockerStatus == nil {
                            Text("Checking Docker availability on the host…")
                                .font(.footnote)
                                .foregroundStyle(.secondary)
                        }
                    }
                    if scope == "repository" {
                        Stepper("Runner count: \(count)", value: $count, in: 1...10)
                        Text(count == 1 ? "Creates one runner." : "Creates a runner group of \(count) runners.")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    } else {
                        Text("Organization runners are created one at a time because each registration token is single-use.")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                }

                if let resultMessage {
                    Section { Label(resultMessage, systemImage: "checkmark.circle").foregroundStyle(.green) }
                }
                if let errorMessage {
                    Section { Label(errorMessage, systemImage: "exclamationmark.triangle").foregroundStyle(.red) }
                }

                Section {
                    Button {
                        Task { await create() }
                    } label: {
                        HStack {
                            Spacer()
                            if creating { ProgressView().padding(.trailing, 6) }
                            Text(creating ? "Creating…" : "Create \(count == 1 ? "runner" : "\(count) runners")")
                                .fontWeight(.semibold)
                            Spacer()
                        }
                    }
                    .disabled(creating || !targetIsValid || (mode == "container" && (containerImage.isEmpty || model.dockerStatus?.available != true)))
                }
            }
            .navigationTitle("New runner")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        Task {
                            await model.refresh()
                            if repository.isEmpty { repository = model.repositories.first?.fullName ?? "" }
                        }
                    } label: { Image(systemName: "arrow.clockwise") }
                        .accessibilityLabel("Reload repositories")
                }
            }
            .task {
                await model.refresh()
                if repository.isEmpty { repository = model.repositories.first?.fullName ?? "" }
            }
        }
    }

    private func create() async {
        creating = true
        errorMessage = nil
        resultMessage = nil
        defer { creating = false }
        let parsedLabels = labels.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        let container = mode == "container" ? ContainerConfig(image: containerImage, extraEnv: ContainerEnvironmentLine.parse(environmentText)) : nil
        do {
            if count == 1 {
                let request = CreateRunnerRequest(
                    repoFullName: scope == "organization" ? "" : repository,
                    scope: scope,
                    organization: scope == "organization" ? normalizedOrganization : nil,
                    manualRegistrationToken: scope == "organization" ? normalizedManualRegistrationToken : nil,
                    name: runnerName.isEmpty ? nil : runnerName,
                    labels: parsedLabels,
                    mode: mode,
                    container: container
                )
                let runner = try await model.createRunner(request)
                manualRegistrationToken = ""
                resultMessage = "Created \(runner.config.name) for \(targetLabel). Registration is running in the background."
            } else {
                let request = CreateBatchRequest(
                    repoFullName: scope == "organization" ? "" : repository,
                    scope: scope,
                    organization: scope == "organization" ? normalizedOrganization : nil,
                    count: count,
                    labels: parsedLabels,
                    mode: mode,
                    container: container
                )
                let batch = try await model.createBatch(request)
                let failures = batch.errors.map { "Runner \($0.index): \($0.error)" }
                resultMessage = "Group \(batch.groupId) for \(targetLabel): \(batch.runners.count) created, \(batch.errors.count) errors." + (failures.isEmpty ? "" : "\n" + failures.joined(separator: "\n"))
            }
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}
