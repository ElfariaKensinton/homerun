import SwiftUI

struct ActivityView: View {
    @EnvironmentObject private var model: AppModel
    @State private var search = ""
    @State private var level = "All"
    @State private var errorMessage: String?

    var body: some View {
        List {
            Section {
                TextField("Search daemon logs", text: $search)
                    .autocorrectionDisabled()
                Picker("Minimum level", selection: $level) {
                    ForEach(["All", "Error", "Warn", "Info", "Debug", "Trace"], id: \.self) {
                        Text($0).tag($0)
                    }
                }
                .pickerStyle(.menu)
                Button("Apply filters") {
                    Task {
                        do { try await model.loadDaemonLogs(level: level, search: search.isEmpty ? nil : search) }
                        catch { errorMessage = error.localizedDescription }
                    }
                }
            } header: {
                Text("Daemon logs")
            } footer: {
                Text("Showing recent host daemon events. New entries appear when the view is refreshed.")
            }

            if let errorMessage {
                Section { Text(errorMessage).foregroundStyle(.red).font(.footnote) }
            }

            Section("Recent events · \(model.daemonLogs.count)") {
                if model.daemonLogs.isEmpty {
                    EmptyStateView(title: "No log entries", message: "No entries match these filters.", symbol: "text.alignleft")
                        .listRowSeparator(.hidden)
                } else {
                    ForEach(model.daemonLogs) { entry in
                        VStack(alignment: .leading, spacing: 5) {
                            HStack {
                                Text(entry.level.uppercased())
                                    .font(.caption2.bold())
                                    .foregroundStyle(levelColor(entry.level))
                                Spacer()
                                Text(formatTimestamp(entry.timestamp))
                                    .font(.caption2)
                                    .foregroundStyle(.secondary)
                            }
                            Text(entry.message)
                                .font(.system(.caption, design: .monospaced))
                                .textSelection(.enabled)
                            Text(entry.target)
                                .font(.caption2)
                                .foregroundStyle(.tertiary)
                        }
                        .padding(.vertical, 3)
                    }
                }
            }
        }
        .navigationTitle("Activity")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    Task {
                        do { try await model.loadDaemonLogs(level: level, search: search.isEmpty ? nil : search) }
                        catch { errorMessage = error.localizedDescription }
                    }
                } label: { Image(systemName: "arrow.clockwise") }
                    .accessibilityLabel("Refresh activity")
            }
        }
        .task {
            do { try await model.loadDaemonLogs(level: level) }
            catch { errorMessage = error.localizedDescription }
        }
        .refreshable {
            do { try await model.loadDaemonLogs(level: level, search: search.isEmpty ? nil : search) }
            catch { errorMessage = error.localizedDescription }
        }
    }

    private func levelColor(_ value: String) -> Color {
        switch value.lowercased() {
        case "error": .red
        case "warn", "warning": .orange
        case "debug", "trace": .secondary
        default: .primary
        }
    }
}
