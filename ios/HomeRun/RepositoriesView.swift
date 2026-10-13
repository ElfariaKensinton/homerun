import SwiftUI

struct RepositoriesView: View {
    @EnvironmentObject private var model: AppModel
    @State private var search = ""
    @State private var localPath = ""
    @State private var message: String?
    @State private var errorMessage: String?

    private var filteredRepos: [RepoInfo] {
        guard !search.isEmpty else { return model.repositories }
        return model.repositories.filter {
            $0.fullName.localizedCaseInsensitiveContains(search)
        }
    }

    private var discovered: [DiscoveredRepo] {
        model.scanResults?.mergedResults ?? []
    }

    var body: some View {
        List {
            Section("Discovery") {
                Button {
                    Task {
                        message = nil
                        errorMessage = nil
                        do {
                            try await model.scanRemote()
                            message = "Remote scan finished."
                        } catch { errorMessage = error.localizedDescription }
                    }
                } label: {
                    Label(model.activeScan ? "Scanning GitHub…" : "Scan GitHub repositories", systemImage: "network")
                }
                .disabled(model.activeScan || !model.authenticated)

                TextField("Host workspace path (optional local scan)", text: $localPath)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                Button {
                    Task {
                        message = nil
                        errorMessage = nil
                        do {
                            try await model.scanLocal(path: localPath)
                            message = "Local workspace scan finished."
                        } catch { errorMessage = error.localizedDescription }
                    }
                } label: {
                    Label("Scan host workspace", systemImage: "folder.badge.gearshape")
                }
                .disabled(model.activeScan || localPath.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)

                if !model.authenticated {
                    Text("Sign in with GitHub in Settings to scan remote repositories.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                if model.activeScan {
                    ProgressView()
                    Text(model.scanProgressText ?? "Scanning…")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                    Button("Cancel scan", role: .destructive) {
                        Task {
                            do { try await model.cancelScan() }
                            catch { errorMessage = error.localizedDescription }
                        }
                    }
                    .disabled(model.activeScanId == nil)
                }
                if let message { Label(message, systemImage: "checkmark.circle").font(.footnote).foregroundStyle(.green) }
                if let errorMessage { Label(errorMessage, systemImage: "exclamationmark.triangle").font(.footnote).foregroundStyle(.red) }
            }

            Section("Discovered repositories · \(discovered.count)") {
                if discovered.isEmpty {
                    Text("Run a scan to find repositories that use self-hosted runners.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(discovered) { repo in
                        VStack(alignment: .leading, spacing: 5) {
                            Text(repo.fullName).font(.headline)
                            HStack {
                                Text(repo.source.capitalized)
                                if !repo.matchedLabels.isEmpty { Text("· Labels: \(repo.matchedLabels.joined(separator: ", "))") }
                            }
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            if !repo.workflowFiles.isEmpty {
                                Text(repo.workflowFiles.joined(separator: ", "))
                                    .font(.caption2.monospaced())
                                    .foregroundStyle(.secondary)
                            }
                            if let path = repo.localPath {
                                Text(path).font(.caption2).foregroundStyle(.tertiary)
                            }
                        }
                        .padding(.vertical, 4)
                    }
                }
                if let date = model.scanResults?.lastScanAt {
                    Text("Last scan: \(formatTimestamp(date))")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
            }

            Section("GitHub repositories · \(filteredRepos.count)") {
                if let repositoryLoadError = model.repositoryLoadError {
                    Label("Repository loading failed: \(repositoryLoadError)", systemImage: "exclamationmark.triangle")
                        .font(.footnote)
                        .foregroundStyle(.red)
                }
                TextField("Filter repositories", text: $search)
                if filteredRepos.isEmpty {
                    Text(!model.authenticated ? "GitHub is not connected." : (model.repositoryLoadError == nil ? "No repositories match the filter." : "Repository list unavailable; see the error above."))
                        .foregroundStyle(.secondary)
                }
                ForEach(filteredRepos) { repo in
                    HStack(spacing: 10) {
                        Image(systemName: repo.isPrivate ? "lock.fill" : "globe")
                            .foregroundStyle(.secondary)
                        VStack(alignment: .leading, spacing: 3) {
                            Text(repo.fullName).font(.headline)
                            Text(repo.isOrg ? "Organization" : "Personal")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        Spacer()
                        if let url = URL(string: repo.htmlUrl) {
                            Link(destination: url) {
                                Image(systemName: "arrow.up.right.square")
                            }
                            .accessibilityLabel("Open repository on GitHub")
                        }
                    }
                }
            }
        }
        .navigationTitle("Repositories")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    Task {
                        do { try await model.loadScanResults() }
                        catch { errorMessage = error.localizedDescription }
                        await model.refresh()
                    }
                } label: { Image(systemName: "arrow.clockwise") }
                .accessibilityLabel("Refresh repositories and scan results")
            }
        }
        .task {
            await model.refresh()
            localPath = model.preferences?.workspacePath ?? localPath
            try? await model.loadScanResults()
        }
        .refreshable { await model.refresh() }
    }
}
