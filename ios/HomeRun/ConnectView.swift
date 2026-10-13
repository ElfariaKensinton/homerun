import SwiftUI

struct ConnectView: View {
    @EnvironmentObject private var model: AppModel
    @State private var endpoint = UserDefaults.standard.string(forKey: "homerun.endpoint") ?? ""
    @State private var token = SecureStore.readToken() ?? ""
    @State private var connecting = false
    @State private var message: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    VStack(alignment: .leading, spacing: 8) {
                        Image(systemName: "server.rack")
                            .font(.system(size: 42, weight: .medium))
                            .foregroundStyle(.indigo)
                        Text("Connect to HomeRun")
                            .font(.title2.bold())
                        Text("Manage the runners on your Mac or Windows host from this device.")
                            .foregroundStyle(.secondary)
                    }
                    .padding(.vertical, 10)
                }

                Section("Daemon endpoint") {
                    TextField("http://macname:47831 or https://host.tailnet.example", text: $endpoint)
                        .textInputAutocapitalization(.never)
                        .keyboardType(.URL)
                        .autocorrectionDisabled()
                    SecureField("Remote API token", text: $token)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    Text("HTTP, HTTPS, and Tailscale MagicDNS hostnames are supported. With direct HTTP, the bearer token and API traffic are unencrypted; use only on a trusted network. The token is stored in the iOS Keychain.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }

                if let message {
                    Section {
                        Label(message, systemImage: "exclamationmark.circle")
                            .foregroundStyle(.red)
                    }
                } else if let lastError = model.lastError {
                    Section {
                        Label(lastError, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.red)
                    }
                }

                Section {
                    Button {
                        Task {
                            connecting = true
                            message = nil
                            defer { connecting = false }
                            do {
                                try await model.connect(endpoint: endpoint, token: token)
                            } catch {
                                message = error.localizedDescription
                            }
                        }
                    } label: {
                        HStack {
                            Spacer()
                            if connecting { ProgressView().padding(.trailing, 6) }
                            Text(connecting ? "Connecting…" : "Connect")
                                .fontWeight(.semibold)
                            Spacer()
                        }
                    }
                    .disabled(connecting || endpoint.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || token.isEmpty)
                }

                Section("Server setup") {
                    Link(destination: URL(string: "https://github.com/ElfariaKensinton/homerun/blob/feat/ios-app/ios/README.md")!) {
                        Label("Read connection and security setup", systemImage: "book")
                    }
                    Text("For direct tailnet access, use http://<Mac-MagicDNS-name>:47831. For encrypted access, use a working Tailscale Serve HTTPS URL. Do not expose port 47831 to the public Internet.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            }
            .navigationTitle("HomeRun")
        }
    }
}
