import SwiftUI

struct ContentView: View {
    @EnvironmentObject private var model: AppModel
    @State private var attemptedRestore = false

    var body: some View {
        Group {
            if model.isConnected {
                MainTabView()
            } else {
                ConnectView()
            }
        }
        .task {
            guard !attemptedRestore else { return }
            attemptedRestore = true
            await model.restoreConnection()
        }
    }
}

struct MainTabView: View {
    var body: some View {
        TabView {
            NavigationStack { DashboardView() }
                .tabItem { Label("Overview", systemImage: "gauge.with.dots.needle.67percent") }
            NavigationStack { RunnersView() }
                .tabItem { Label("Runners", systemImage: "server.rack") }
            NavigationStack { RepositoriesView() }
                .tabItem { Label("Repositories", systemImage: "folder") }
            NavigationStack { ActivityView() }
                .tabItem { Label("Activity", systemImage: "text.alignleft") }
            NavigationStack { SettingsView() }
                .tabItem { Label("Settings", systemImage: "gearshape") }
        }
    }
}
