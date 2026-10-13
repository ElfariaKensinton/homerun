import SwiftUI

@main
struct HomeRunApp: App {
    @StateObject private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(model)
                .tint(.indigo)
        }
    }
}
